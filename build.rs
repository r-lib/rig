
use clap_complete::shells::Bash;
use clap_complete::shells::Zsh;
use clap_complete::shells::PowerShell;
use clap_mangen::Man;

use std::env;
use std::io::Error;
use std::path::Path;

include!("src/args.rs");

fn main() -> Result<(), Error> {
    let outdir = match env::var_os("OUT_DIR") {
        None => return Ok(()),
        Some(outdir) => outdir,
    };

    let mut app = rim_app();
    let name = "rim".to_string();

    let path = clap_complete::generate_to(Bash, &mut app, &name, &outdir);
    println!("bash completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(Zsh, &mut app, &name, &outdir);
    println!("zsh completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(PowerShell, &mut app, &name, &outdir);
    println!("powershell completion file is generated: {:?}", path);

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let app = app
            .name("rim")
            .about("R Installation Manager")
            .long_about(HELP_ABOUT);
        let man = clap_mangen::Man::new(app);
        let mut buffer: Vec<u8> = Default::default();
        man.render_title(&mut buffer)?;
        man.render_name_section(&mut buffer)?;
        man.render_synopsis_section(&mut buffer)?;
        man.render_description_section(&mut buffer)?;
        man.render_options_section(&mut buffer)?;
        man.render_subcommands_section(&mut buffer)?;
        man.render_extra_section(&mut buffer)?;

        let outdir = Path::new(&outdir);
        std::fs::write(outdir.join("rim.1"), buffer)?;
    }

    Ok(())
}
