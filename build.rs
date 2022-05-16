
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


#[cfg(target_os = "windows")]
{
    static_vcruntime::metabuild();
}

    let mut app = rig_app();
    let name = "rig".to_string();

    let path = clap_complete::generate_to(Bash, &mut app, &name, &outdir);
    println!("bash completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(Zsh, &mut app, &name, &outdir);
    println!("zsh completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(PowerShell, &mut app, &name, &outdir);
    println!("powershell completion file is generated: {:?}", path);

    Ok(())
}
