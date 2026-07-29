use pkg2mpkg_core::sanitize_mobile_properties;
use serde_json::{Map, json};

#[test]
fn strips_only_the_windows_mobile_property_blacklist() {
    let input: Map<String, serde_json::Value> = serde_json::from_value(json!({
        "alignment": {"value": "center"},
        "alignmentx": {"value": 0.5},
        "pluginledextensionsenableleds": {"value": true},
        "wec_hue": {"value": 0.8},
        "rate": {"value": 2.0},
        "dino": {"value": "vita"},
        "Alignment": {"value": "user-defined-different-key"}
    }))
    .unwrap();

    let output = sanitize_mobile_properties(&input);
    assert_eq!(output.len(), 2);
    assert_eq!(output["dino"]["value"], "vita");
    assert_eq!(output["Alignment"]["value"], "user-defined-different-key");
}

#[test]
fn sanitizer_does_not_mutate_the_input_or_nested_scene_data() {
    let input: Map<String, serde_json::Value> = serde_json::from_value(json!({
        "rate": {"value": 2.0},
        "custom": {"rate": {"value": 3.0}}
    }))
    .unwrap();
    let original = input.clone();

    let output = sanitize_mobile_properties(&input);
    assert_eq!(input, original);
    assert_eq!(output["custom"]["rate"]["value"], 3.0);
}
