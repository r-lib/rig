
use clap_complete::shells::Bash;
use clap_complete::shells::Zsh;
use clap_complete::shells::PowerShell;
use std::env;
use std::io::Error;

include!("src/args.rs");

fn main() -> Result<(), Error> {
    let outdir = match env::var_os("OUT_DIR") {
        None => return Ok(()),
        Some(outdir) => outdir,
    };

    let mut app = rim_app();
    let name = "rim".to_string();

    let path = clap_complete::generate_to(Bash, &mut app, &name, &outdir);
    println!("cargo:warning=bash completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(Zsh, &mut app, &name, &outdir);
    println!("cargo:warning=zsh completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(PowerShell, &mut app, &name, &outdir);
    println!("cargo:warning=powershell completion file is generated: {:?}", path);

    Ok(())
}
