//! One isolated native plugin helper; the parent owns its control socket.
fn main() {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<_> = std::env::args().collect();
        let result = if args.len() == 3 && args[1] == "--control-fd" {
            args[2]
                .parse::<i32>()
                .map_err(|e| e.to_string())
                .and_then(|fd| {
                    // SAFETY: the executable contract transfers this inherited descriptor once.
                    unsafe { clausters_plugin_host::runtime::helper_main(fd) }
                })
        } else {
            Err("usage: clausters-plugin-host --control-fd <fd>".into())
        };
        if let Err(error) = result {
            eprintln!("plugin helper: {error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("native plugin hosting currently requires Linux");
        std::process::exit(1);
    }
}
