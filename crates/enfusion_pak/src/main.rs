use std::{ffi::OsStr, path::PathBuf, rc::Rc};

use clap::Parser;
use enfusion_pak::PakFile;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    file: PathBuf,
}

pub fn add_num(integers: &mut Vec<u32>) {
    integers.push(0);
}

fn main() -> color_eyre::Result<()> {
    let mut args = Args::parse();

    if !args.file.exists() {
        println!("File does not exist");
        return Ok(());
    }

    if args.file.is_dir() {
        println!("it's a dir!");
        for entry in std::fs::read_dir(&args.file)? {
            let entry = entry?;
            let path = entry.path();
            if let Some("pak") = path.extension().and_then(OsStr::to_str) {
                println!("{:?}", PakFile::parse(&path));
            }
        }
    } else {
        println!("{:?}", PakFile::parse(&args.file)?);
    }

    Ok(())
}
