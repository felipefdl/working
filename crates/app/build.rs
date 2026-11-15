//! Embeds the app icon (`assets/working.ico`, from `scripts/logo.sh`) and the version information Explorer shows in the
//! Windows executable. Nothing to do for other targets.

fn main() {
  println!("cargo:rerun-if-changed=assets/working.ico");
  #[cfg(windows)]
  if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
    embed_resources();
  }
}

#[cfg(windows)]
fn embed_resources() {
  let compiled = winresource::WindowsResource::new()
    .set_icon("assets/working.ico")
    .set("ProductName", "Working")
    .set("FileDescription", "Working")
    .set("LegalCopyright", "MIT License")
    .compile();
  if let Err(error) = compiled {
    println!("cargo::error=could not embed the Windows resources: {error}");
  }
}
