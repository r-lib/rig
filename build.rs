
use clap_generate::generators::Bash;
use clap_generate::generators::Zsh;
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

    let path = clap_generate::generate_to(Bash, &mut app, &name, &outdir);
    println!("cargo:warning=bash completion file is generated: {:?}", path);

    let path = clap_generate::generate_to(Zsh, &mut app, &name, &outdir);
    println!("cargo:warning=zsh completion file is generated: {:?}", path);

    Ok(())
}
