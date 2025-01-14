mod common;
mod debugger;
mod info;

use common::{with_device, with_dump, CliError};
use debugger::CliState;

use probe_rs::{
    debug::DebugInfo,
    memory::MI,
    probe::{
        daplink,
        debug_probe::DebugProbeInfo,
        flash::download::{FileDownloader, Format},
        stlink,
    },
};

use capstone::{arch::arm::ArchMode, prelude::*, Capstone, Endian};
use colored::*;
use memmap;
use rustyline::Editor;
use structopt::StructOpt;

use std::fs;
use std::num::ParseIntError;
use std::path::PathBuf;
use std::time::Instant;

fn parse_hex(src: &str) -> Result<u32, ParseIntError> {
    u32::from_str_radix(src, 16)
}

#[derive(StructOpt)]
#[structopt(
    name = "Probe-rs CLI",
    about = "A CLI for on top of the debug probe capabilities provided by probe-rs",
    author = "Noah Hüsser <yatekii@yatekii.ch> / Dominik Böhi <dominik.boehi@gmail.ch>"
)]
enum CLI {
    /// List all connected debug probes
    #[structopt(name = "list")]
    List {},
    /// Gets infos about the selected debug probe and connected target
    #[structopt(name = "info")]
    Info {
        #[structopt(flatten)]
        shared: SharedOptions,
    },
    /// Resets the target attached to the selected debug probe
    #[structopt(name = "reset")]
    Reset {
        #[structopt(flatten)]
        shared: SharedOptions,

        /// Whether the reset pin should be asserted or deasserted. If left open, just pulse it
        assert: Option<bool>,
    },
    #[structopt(name = "debug")]
    Debug {
        #[structopt(flatten)]
        shared: SharedOptions,

        #[structopt(long, parse(from_os_str))]
        /// Dump file to debug
        dump: Option<PathBuf>,

        #[structopt(long, parse(from_os_str))]
        /// Binary to debug
        exe: Option<PathBuf>,
    },
    /// Dump memory from attached target
    #[structopt(name = "dump")]
    Dump {
        #[structopt(flatten)]
        shared: SharedOptions,

        /// The address of the memory to dump from the target (in hexadecimal without 0x prefix)
        #[structopt(parse(try_from_str = "parse_hex"))]
        loc: u32,
        /// The amount of memory (in words) to dump
        words: u32,
    },
    /// Download memory to attached target
    #[structopt(name = "download")]
    Download {
        #[structopt(flatten)]
        shared: SharedOptions,

        /// The path to the file to be downloaded to the flash
        path: String,
    },
    #[structopt(name = "trace")]
    Trace {
        #[structopt(flatten)]
        shared: SharedOptions,

        /// The address of the memory to dump from the target (in hexadecimal without 0x prefix)
        #[structopt(parse(try_from_str = "parse_hex"))]
        loc: u32,
    },
}

/// Shared options for all commands which use a specific probe
#[derive(StructOpt)]
struct SharedOptions {
    /// The number associated with the debug probe to use
    #[structopt(long = "probe-index")]
    n: Option<usize>,

    /// The target to be selected.
    #[structopt(short, long)]
    target: Option<String>,
}

fn main() {
    // Initialize the logging backend.
    pretty_env_logger::init();

    let matches = CLI::from_args();

    let cli_result = match matches {
        CLI::List {} => list_connected_devices(),
        CLI::Info { shared } => crate::info::show_info_of_device(&shared),
        CLI::Reset { shared, assert } => reset_target_of_device(&shared, assert),
        CLI::Debug { shared, exe, dump } => debug(&shared, exe, dump),
        CLI::Dump { shared, loc, words } => dump_memory(&shared, loc, words),
        CLI::Download { shared, path } => download_program_fast(&shared, &path),
        CLI::Trace { shared, loc } => trace_u32_on_target(&shared, loc),
    };

    if let Err(e) = cli_result {
        if let CliError::TargetSelectionError(e) = e {
            eprintln!("    {} {}", "Error".red().bold(), e);
        } else {
            eprintln!("Error processing command: {}", e);
        }
        std::process::exit(1);
    }
}

fn list_connected_devices() -> Result<(), CliError> {
    let links = get_connected_devices();

    if !links.is_empty() {
        println!("The following devices were found:");
        links
            .iter()
            .enumerate()
            .for_each(|(num, link)| println!("[{}]: {:?}", num, link));
    } else {
        println!("No devices were found.");
    }

    Ok(())
}

fn dump_memory(shared_options: &SharedOptions, loc: u32, words: u32) -> Result<(), CliError> {
    with_device(shared_options, |mut session| {
        let mut data = vec![0 as u32; words as usize];

        // Start timer.
        let instant = Instant::now();

        // let loc = 220 * 1024;

        session.probe.read_block32(loc, &mut data.as_mut_slice())?;
        // Stop timer.
        let elapsed = instant.elapsed();

        // Print read values.
        for word in 0..words {
            println!(
                "Addr 0x{:08x?}: 0x{:08x}",
                loc + 4 * word,
                data[word as usize]
            );
        }
        // Print stats.
        println!("Read {:?} words in {:?}", words, elapsed);

        Ok(())
    })
}

fn download_program_fast(shared_options: &SharedOptions, path: &str) -> Result<(), CliError> {
    with_device(shared_options, |mut session| {
        // Start timer.
        // let instant = Instant::now();

        let fd = FileDownloader::new();
        let mm = session.target.memory_map.clone();

        fd.download_file(&mut session, std::path::Path::new(&path), Format::Elf, &mm)?;

        Ok(())
    })
}

fn reset_target_of_device(
    shared_options: &SharedOptions,
    _assert: Option<bool>,
) -> Result<(), CliError> {
    with_device(shared_options, |mut session| {
        session.probe.target_reset()?;

        Ok(())
    })
}

fn trace_u32_on_target(shared_options: &SharedOptions, loc: u32) -> Result<(), CliError> {
    use scroll::Pwrite;
    use std::io::prelude::*;
    use std::thread::sleep;
    use std::time::Duration;

    let mut xs = vec![];
    let mut ys = vec![];

    let start = Instant::now();

    with_device(shared_options, |mut session| {
        loop {
            // Prepare read.
            let elapsed = start.elapsed();
            let instant = elapsed.as_secs() * 1000 + u64::from(elapsed.subsec_millis());

            // Read data.
            let value: u32 = session.probe.read32(loc)?;

            xs.push(instant);
            ys.push(value);

            // Send value to plot.py.
            // Unwrap is safe as there is always an stdin in our case!
            let mut buf = [0 as u8; 8];
            // Unwrap is safe!
            buf.pwrite(instant, 0).unwrap();
            buf.pwrite(value, 4).unwrap();
            std::io::stdout().write_all(&buf)?;

            std::io::stdout().flush()?;

            // Schedule next read.
            let elapsed = start.elapsed();
            let instant = elapsed.as_secs() * 1000 + u64::from(elapsed.subsec_millis());
            let poll_every_ms = 50;
            let time_to_wait = poll_every_ms - instant % poll_every_ms;
            sleep(Duration::from_millis(time_to_wait));
        }
    })
}

fn get_connected_devices() -> Vec<DebugProbeInfo> {
    let mut links = daplink::tools::list_daplink_devices();
    links.extend(stlink::tools::list_stlink_devices());
    links
}

fn debug(
    shared_options: &SharedOptions,
    exe: Option<PathBuf>,
    dump: Option<PathBuf>,
) -> Result<(), CliError> {
    // try to load debug information
    let debug_data = exe
        .and_then(|p| fs::File::open(&p).ok())
        .and_then(|file| unsafe { memmap::Mmap::map(&file).ok() });

    let runner = |session| {
        let cs = Capstone::new()
            .arm()
            .mode(ArchMode::Thumb)
            .endian(Endian::Little)
            .build()
            .unwrap();

        let di = debug_data.as_ref().map(|mmap| DebugInfo::from_raw(&*mmap));

        let cli = debugger::DebugCli::new();

        let mut cli_data = debugger::CliData {
            session,
            debug_info: di,
            capstone: cs,
        };

        let mut rl = Editor::<()>::new();

        loop {
            let readline = rl.readline(">> ");
            match readline {
                Ok(line) => {
                    let history_entry: &str = line.as_ref();
                    rl.add_history_entry(history_entry);
                    let cli_state = cli.handle_line(&line, &mut cli_data)?;

                    match cli_state {
                        CliState::Continue => (),
                        CliState::Stop => return Ok(()),
                    }
                }
                Err(e) => {
                    use rustyline::error::ReadlineError;

                    match e {
                        // For end of file and ctrl-c, we just quit
                        ReadlineError::Eof | ReadlineError::Interrupted => return Ok(()),
                        actual_error => {
                            // Show error message and quit
                            println!("Error handling input: {:?}", actual_error);
                            return Ok(());
                        }
                    }
                }
            }
        }
    };

    match dump {
        None => with_device(shared_options, &runner),
        Some(p) => with_dump(shared_options, &p, &runner),
    }
}
