#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Group {
    Main,
    Session,
}

pub fn classify(path: &str) -> Group {
    if is_test(path) || is_generated(path) {
        Group::Session
    } else {
        Group::Main
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_test(path: &str) -> bool {
    if path
        .split('/')
        .any(|s| matches!(s, "tests" | "test" | "__tests__" | "__test__" | "e2e"))
    {
        return true;
    }
    let f = file_name(path);
    f.starts_with("test_") || f.contains("_test.") || f.contains(".spec.") || f.contains(".test.")
}

fn is_generated(path: &str) -> bool {
    if path.split('/').any(|s| s == "generated") {
        return true;
    }
    let f = file_name(path);
    if f.contains(".generated.") || f.starts_with("generated_") || f.contains("_generated.") {
        return true;
    }
    if f.ends_with(".min.js") || f.ends_with(".min.css") || f.contains(".pb.") {
        return true;
    }
    matches!(
        f,
        "Cargo.lock"
            | "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "Gemfile.lock"
            | "poetry.lock"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_match_screenshot_groups() {
        assert_eq!(classify("CHANGELOG.md"), Group::Main);
        assert_eq!(classify("src/color.rs"), Group::Main);
        assert_eq!(classify("src/util.rs"), Group::Main);
        assert_eq!(classify("tests/color.rs"), Group::Session);
        assert_eq!(classify("src/generated/bindings.rs"), Group::Session);
        assert_eq!(classify("Cargo.lock"), Group::Session);
        assert_eq!(classify("src/net_test.go"), Group::Session);
        assert_eq!(classify("web/app.spec.ts"), Group::Session);
    }
}
