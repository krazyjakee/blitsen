//! Build-time Node-API symbol configuration.

fn main() {
    napi_build::setup();

    // Apple's linker otherwise records the absolute `-o` path as this
    // cdylib's LC_ID_DYLIB. Two clean builds in different checkout paths then
    // differ in both the load-command size and its string even though Node
    // loads the addon by filename. Give the staged addon one stable identity;
    // codesign still runs later and is deliberately outside the unsigned
    // reproducibility boundary.
    #[cfg(target_os = "macos")]
    println!("cargo::rustc-cdylib-link-arg=-Wl,-install_name,@rpath/blitsen.node");
}
