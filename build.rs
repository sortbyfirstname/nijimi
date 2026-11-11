fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=um-bee-ants.rc");
        println!("cargo:rerun-if-changed=assets/tresbient.ico");
        embed_resource::compile("um-bee-ants.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to embed the um-bee-ants icon resource");
    }
}
