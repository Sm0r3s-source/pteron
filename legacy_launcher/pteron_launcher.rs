// Pteron 69 starts "Pteron.exe" after an update. This tiny program hands over to Sirocco.exe,
// so old shortcuts and the old updater keep working. It is built by the update script.
#![windows_subsystem = "windows"]
fn main() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let _ = std::process::Command::new(dir.join("Sirocco.exe")).args(std::env::args_os().skip(1)).spawn();
}
