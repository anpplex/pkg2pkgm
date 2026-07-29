mod export;
mod inspect;
mod verify;

use pkg2mpkg_core::Result;

use crate::args::Command;

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Inspect { input, json } => inspect::run(&input, json),
        Command::Verify { input, json } => verify::run(&input, json),
        Command::Export {
            input,
            output,
            profile,
            compression,
            reduction,
            we_runtime,
            wine,
            winepath,
            replace,
            dry_run,
            json,
        } => export::run(
            &input,
            export::ExportOptions {
                output,
                profile,
                compression,
                reduction,
                we_runtime,
                wine,
                winepath,
                replace,
                dry_run,
                json,
            },
        ),
    }
}
