use anyhow::Context;



fn main() -> Result<(), Box<dyn std::error::Error>> {

    built::write_built_file().expect("Failed to acquire build-time information");

    {
        println!("cargo:rerun-if-changed=build.rs");
        // print_info();
        write_version();
    }
 
    Ok(())
}

fn write_version() {
    use std::io::Write;

    let src = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let pkg_ver = std::env::var("CARGO_PKG_VERSION").unwrap();
    

    let (hash_or_tag, _dirty) = built::util::get_repo_description(src.as_ref())
    .with_context(||format!("src_path [{}], pkg_ver [{}]", src, pkg_ver))
    .unwrap()
    .with_context(||format!("src_path [{}], pkg_ver [{}]", src, pkg_ver))
    .unwrap();

    let dst = format!("{}/out_version.txt", src);
    let mut built_file = std::fs::File::create(&dst).unwrap();

    // 1.0.8.8f01411
    built_file.write_all(format!("{}.{}",pkg_ver, hash_or_tag).as_bytes()).unwrap();
}
