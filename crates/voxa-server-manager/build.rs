fn main() {
    println!("cargo:rerun-if-changed=../../assets/voxa-server.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/voxa-server.ico")
            .set("ProductName", "Voxa Server")
            .set("FileDescription", "Voxa Server Manager")
            .set("CompanyName", "Voxa Project")
            .set("LegalCopyright", "Voxa contributors - MIT OR Apache-2.0");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
