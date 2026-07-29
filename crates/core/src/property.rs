use serde_json::{Map, Value};

const MOBILE_PROPERTY_BLACKLIST: &[&str] = &[
    "alignment",
    "alignmentx",
    "alignmenty",
    "alignmentz",
    "alignmentposition",
    "alignmentfliph",
    "pluginledextensionsenableleds",
    "wec_e",
    "wec_brs",
    "wec_con",
    "wec_sat",
    "wec_hue",
    "rate",
];

pub fn sanitize_mobile_properties(input: &Map<String, Value>) -> Map<String, Value> {
    input
        .iter()
        .filter(|(key, _)| !MOBILE_PROPERTY_BLACKLIST.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
