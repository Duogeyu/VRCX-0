//! Diagnostic: call the same app-data-dir resolution the Tauri app uses.

fn main() {
    println!("APPDATA = {:?}", std::env::var_os("APPDATA"));
    println!("LOCALAPPDATA = {:?}", std::env::var_os("LOCALAPPDATA"));
    println!("cwd = {:?}", std::env::current_dir());
    match vrcx_0_platform::app_paths::resolve_app_data_dir() {
        Ok(resolution) => {
            println!("OK current_dir = {}", resolution.current_dir.display());
            println!("   default_dir = {}", resolution.default_dir.display());
            println!("   source = {:?}", resolution.source);
        }
        Err(error) => println!("ERR resolve_app_data_dir: {error}"),
    }
}
