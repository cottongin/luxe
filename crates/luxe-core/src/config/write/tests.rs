use super::*;

/// The OS error is said once, whether a caller prints the message alone or walks the chain.
#[test]
fn a_failed_write_names_its_cause_once() {
    let path = PathBuf::from("config.json");
    let error = ConfigWriteError::Io { path, error: std::io::Error::other("disk on fire") };
    assert_eq!(error.to_string(), "writing config.json: disk on fire");
    let chain = crate::error_cause::chain(&error);
    assert_eq!(chain.matches("disk on fire").count(), 1, "{chain}");
}

fn with_home(test: impl FnOnce(&Path)) {
    let _guard =
        super::super::XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = tempfile::tempdir().expect("temp dir");
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    test(home.path());
    std::env::remove_var("XDG_CONFIG_HOME");
}

fn seed(text: &str) -> PathBuf {
    let path = Config::global_path();
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, text).expect("seed");
    path
}

/// The literal a reader would have: keys this build knows, one it does not, and an order of
/// their own. Setting one value must change that one line and nothing else.
#[test]
fn a_write_changes_one_value_and_keeps_the_rest_byte_for_byte() {
    with_home(|_| {
        let path = seed(
            r#"{
  "$schema": "./config.schema.json",
  "theme": "nord",
  "a_key_from_a_newer_build": {
    "nested": [
      1,
      2
    ]
  },
  "tools": {
    "disabled": [
      "screenshot"
    ]
  }
}
"#,
        );
        Config::set_global(&["tools", "disabled"], serde_json::json!(["screenshot", "debug"]))
            .expect("write");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            r#"{
  "$schema": "./config.schema.json",
  "theme": "nord",
  "a_key_from_a_newer_build": {
    "nested": [
      1,
      2
    ]
  },
  "tools": {
    "disabled": [
      "screenshot",
      "debug"
    ]
  }
}
"#
        );
    });
}

#[test]
fn a_first_write_creates_the_file_and_its_schema() {
    with_home(|_| {
        let path = Config::set_global(&["conversation", "growth"], "fill".into()).expect("write");
        assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                "{\n  \"$schema\": \"./config.schema.json\",\n  \"conversation\": {\n    \"growth\": \"fill\"\n  }\n}\n"
            );
        let schema = path.parent().expect("dir").join("config.schema.json");
        assert!(schema.exists(), "an editor can complete what was just written");
    });
}

/// A settings surface writing into a file the reader is halfway through editing would throw
/// their edit away. It says so instead, and the file is untouched.
#[test]
fn a_file_that_will_not_parse_is_refused_rather_than_replaced() {
    with_home(|_| {
        for bad in ["{ \"theme\": ", "[1, 2]"] {
            let path = seed(bad);
            let error = Config::set_global(&["mouse"], false.into()).expect_err(bad);
            assert!(matches!(error, ConfigWriteError::Unparseable { .. }), "{error}");
            assert_eq!(std::fs::read_to_string(&path).expect("read"), bad, "left alone");
        }
    });
}

/// Resetting a setting puts the default back by removing the key, and an object left empty by
/// that goes too rather than lingering as `"tools": {}`.
#[test]
fn unsetting_removes_the_key_and_any_object_it_empties() {
    with_home(|_| {
        let path = seed(
                "{\"theme\":\"nord\",\"tools\":{\"disabled\":[\"debug\"]},\"dock\":{\"mode\":\"auto\",\"position\":\"left\"}}",
            );
        Config::unset_global(&["tools", "disabled"]).expect("unset");
        Config::unset_global(&["dock", "mode"]).expect("unset");
        Config::unset_global(&["not", "there"]).expect("a missing key is not an error");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(written, serde_json::json!({ "theme": "nord", "dock": { "position": "left" } }));
    });
}

#[test]
fn a_tool_is_turned_off_and_back_on_without_disturbing_its_neighbors() {
    with_home(|_| {
        let path = seed("{\"tools\":{\"disabled\":[\"screenshot\"],\"format_on_save\":false}}");
        Config::set_tool_enabled_globally("debug", false).expect("off");
        Config::set_tool_enabled_globally("debug", false).expect("off twice is still once");
        let read = || -> Value {
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json")
        };
        assert_eq!(read()["tools"]["disabled"], serde_json::json!(["screenshot", "debug"]));
        Config::set_tool_enabled_globally("screenshot", true).expect("on");
        Config::set_tool_enabled_globally("debug", true).expect("on");
        assert_eq!(read(), serde_json::json!({ "tools": { "format_on_save": false } }));
    });
}

/// A value set by the project wins over the global one, and says so; a value nobody set is the
/// default.
#[test]
fn a_lookup_names_the_layer_that_won() {
    let layers = ConfigLayers {
        global: Some(serde_json::json!({ "mouse": false, "dock": { "mode": "auto" } })),
        project: Some(serde_json::json!({ "dock": { "mode": "hidden" }, "bell": null })),
    };
    assert_eq!(layers.lookup(&["mouse"]), (Layer::Global, Some(&Value::Bool(false))));
    assert_eq!(layers.lookup(&["dock", "mode"]), (Layer::Project, Some(&"hidden".into())));
    assert_eq!(layers.lookup(&["bell"]), (Layer::Default, None), "null is not a value");
    assert_eq!(layers.lookup(&["glyphs"]), (Layer::Default, None));
}
