//! Compiles the bundled icons into a GResource the binary carries.

fn main() {
    glib_build_tools::compile_resources(
        &["icons", "data"],
        "pipedeck.gresource.xml",
        "pipedeck.gresource",
    );
}
