use clap_complete::shells::{Bash, Elvish, Fish, PowerShell, Zsh};
use std::env;
use std::io::Error;

// `src/args.rs` is compiled into the build script as well, so the modules it
// uses have to exist here, too.
#[allow(dead_code)]
#[path = "src/pager.rs"]
mod pager;

include!("src/args.rs");

fn main() -> Result<(), Error> {
    #[cfg(target_os = "windows")]
    {
        static_vcruntime::metabuild();
    }

    // `rig_app()` builds the whole clap command tree in a single function, so in
    // an unoptimized build its stack frame is larger than the 1 MB main thread
    // stack we get on Windows. Do the work on a thread with a bigger stack.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(generate_completions)
        .expect("failed to spawn completion generator thread")
        .join()
        .expect("completion generator thread panicked")
}

fn generate_completions() -> Result<(), Error> {
    let outdir = match env::var_os("OUT_DIR") {
        None => return Ok(()),
        Some(outdir) => outdir,
    };

    let mut app = rig_app();
    let name = "rig".to_string();

    let path = clap_complete::generate_to(Bash, &mut app, &name, &outdir);
    println!("bash completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(Elvish, &mut app, &name, &outdir);
    println!("elvish completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(Fish, &mut app, &name, &outdir);
    println!("fish completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(PowerShell, &mut app, &name, &outdir);
    println!("powershell completion file is generated: {:?}", path);

    let path = clap_complete::generate_to(Zsh, &mut app, &name, &outdir);
    println!("zsh completion file is generated: {:?}", path);

    Ok(())
}
