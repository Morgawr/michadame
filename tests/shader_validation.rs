use std::path::PathBuf;
use std::process::Command;

#[test]
fn test_glsl_shaders_compile() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let shaders_dir = PathBuf::from(manifest_dir)
        .join("src")
        .join("video")
        .join("shaders");

    // Attempt to check if glslangValidator is installed
    let probe = Command::new("glslangValidator").arg("--version").output();
    if probe.is_err() {
        println!("glslangValidator not found. Skipping static shader compilation test.");
        return;
    }

    let mut failed_shaders = Vec::new();

    let entries = std::fs::read_dir(shaders_dir).expect("Failed to read shaders directory");
    let mut total_count = 0;

    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("glsl") {
            continue;
        }

        let file_name = path.file_name().unwrap().to_str().unwrap();
        let stage = if file_name.starts_with("cs_") {
            "comp"
        } else if file_name.starts_with("fs_") {
            "frag"
        } else if file_name.starts_with("vs_") {
            "vert"
        } else {
            println!(
                "Warning: Unknown shader stage for file {}, skipping.",
                file_name
            );
            continue;
        };

        total_count += 1;

        let output = Command::new("glslangValidator")
            .arg("-S")
            .arg(stage)
            .arg(&path)
            .output()
            .expect("Failed to execute glslangValidator");

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            failed_shaders.push(format!(
                "Shader {} failed to compile:\n{}\n{}",
                file_name, stdout, stderr
            ));
        }
    }

    println!(
        "glslangValidator successfully validated {} shaders.",
        total_count
    );

    if !failed_shaders.is_empty() {
        panic!(
            "{} shaders failed to compile:\n\n{}",
            failed_shaders.len(),
            failed_shaders.join("\n\n")
        );
    }
}
