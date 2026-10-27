use std::path::{Path, PathBuf};
use std::process::Command;

const EXPECTED_MIN_SHADERS: usize = 57;

fn shaders_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("video")
        .join("shaders")
}

fn validate_shaders_in_dir(shaders_dir: &Path, validator_bin: &str) -> Result<usize, String> {
    match Command::new(validator_bin).arg("--version").output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(format!(
                "glslangValidator ('{validator_bin}') is required for shader validation, \
                 but '--version' exited with status {}: {stdout} {stderr}",
                output.status
            ));
        }
        Err(err) => {
            return Err(format!(
                "glslangValidator ('{validator_bin}') is required for shader validation, \
                 but failed to launch: {err}"
            ));
        }
    }

    let entries = std::fs::read_dir(shaders_dir).map_err(|err| {
        format!(
            "Failed to read shaders directory '{}': {err}",
            shaders_dir.display()
        )
    })?;

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "Failed to read directory entry in '{}': {err}",
                shaders_dir.display()
            )
        })?;
        paths.push(entry.path());
    }
    paths.sort();

    let mut failed_shaders = Vec::new();
    let mut total_count = 0;

    for path in paths {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<invalid-utf8>");

        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("glsl") {
            failed_shaders.push(format!(
                "Unexpected non-.glsl file in shaders directory: {file_name}"
            ));
            continue;
        }

        let stage = if file_name.starts_with("cs_") {
            "comp"
        } else if file_name.starts_with("fs_") {
            "frag"
        } else if file_name.starts_with("vs_") {
            "vert"
        } else {
            failed_shaders.push(format!(
                "Unknown shader stage for file {file_name} (expected cs_, fs_, or vs_ prefix)"
            ));
            continue;
        };

        total_count += 1;

        let output = match Command::new(validator_bin)
            .arg("-S")
            .arg(stage)
            .arg(&path)
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                failed_shaders.push(format!(
                    "Failed to execute {validator_bin} for {file_name}: {err}"
                ));
                continue;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            failed_shaders.push(format!(
                "Shader {file_name} failed to compile:\n{stdout}\n{stderr}"
            ));
        }
    }

    if !failed_shaders.is_empty() {
        return Err(format!(
            "{} shaders failed validation:\n\n{}",
            failed_shaders.len(),
            failed_shaders.join("\n\n")
        ));
    }

    if total_count == 0 {
        return Err(format!(
            "No .glsl shaders were found in '{}'",
            shaders_dir.display()
        ));
    }

    Ok(total_count)
}

#[test]
fn test_glsl_shaders_compile() {
    let dir = shaders_dir();
    let total_count =
        validate_shaders_in_dir(&dir, "glslangValidator").expect("Shader validation failed");

    println!(
        "glslangValidator successfully validated {} shaders.",
        total_count
    );

    assert!(
        total_count >= EXPECTED_MIN_SHADERS,
        "Expected at least {EXPECTED_MIN_SHADERS} validated shaders, found {total_count}"
    );
}

#[test]
fn test_missing_validator_fails() {
    let dir = shaders_dir();
    let result = validate_shaders_in_dir(&dir, "nonexistent_glslang_validator_binary_xyz");
    let err = result.expect_err("Expected missing validator binary to fail shader validation");
    assert!(
        err.contains("glslangValidator"),
        "Unexpected error message: {err}"
    );
}

#[test]
fn test_unknown_shader_stage_fails() {
    let temp = tempfile::tempdir().expect("Failed to create temp shader dir");
    let valid_vs =
        "#version 330 core\nvoid main() {\n    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);\n}\n";
    std::fs::write(temp.path().join("vs_test.glsl"), valid_vs)
        .expect("Failed to write vs_test.glsl");
    std::fs::write(temp.path().join("xx_unknown.glsl"), valid_vs)
        .expect("Failed to write xx_unknown.glsl");

    let result = validate_shaders_in_dir(temp.path(), "glslangValidator");
    let err = result.expect_err("Expected unknown shader stage prefix to fail validation");
    assert!(
        err.contains("xx_unknown.glsl"),
        "Expected error to mention xx_unknown.glsl, got: {err}"
    );
}

#[test]
fn test_empty_shader_dir_fails() {
    let temp = tempfile::tempdir().expect("Failed to create empty temp shader dir");
    let result = validate_shaders_in_dir(temp.path(), "glslangValidator");
    assert!(
        result.is_err(),
        "Expected empty shader directory to fail validation"
    );
}
