//! Lenient argument shapes some models send for the file tools: `null` for a
//! parameter they mean to leave out, numbers and booleans written as strings,
//! a whole number written as `2.0`. They are turned into the schema's types
//! before the arguments are checked against it.

use serde_json::{Map, Value};

/// Remove each named parameter whose value is `null`.
pub fn drop_nulls(object: &mut Map<String, Value>, names: &[&str]) {
    for name in names {
        if object.get(*name).is_some_and(Value::is_null) {
            object.remove(*name);
        }
    }
}

/// Turn `"12"` or `12.0` into the integer `12`. Anything else stays, so the
/// schema check reports it.
pub fn integer(object: &mut Map<String, Value>, name: &str) {
    let Some(value) = object.get(name) else {
        return;
    };
    let whole = match value {
        Value::String(text) => text.trim().parse::<u64>().ok(),
        Value::Number(number) if number.as_u64().is_none() => number
            .as_f64()
            .filter(|float| float.fract() == 0.0 && *float >= 0.0 && *float <= u64::MAX as f64)
            .map(|float| float as u64),
        _ => None,
    };
    if let Some(whole) = whole {
        object.insert(name.to_string(), Value::from(whole));
    }
}

/// Turn `"true"` or `"false"` into a boolean.
pub fn boolean(object: &mut Map<String, Value>, name: &str) {
    let flag = match object.get(name) {
        Some(Value::String(text)) if text.trim().eq_ignore_ascii_case("true") => true,
        Some(Value::String(text)) if text.trim().eq_ignore_ascii_case("false") => false,
        _ => return,
    };
    object.insert(name.to_string(), Value::Bool(flag));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lenient_shapes_become_the_schema_types() {
        let mut args = json!({
            "offset": "12",
            "limit": 3.0,
            "allow_large": "true",
            "replace_all": "False",
            "gone": null,
            "bad": "x",
        });
        let object = args.as_object_mut().unwrap();
        drop_nulls(object, &["gone"]);
        integer(object, "offset");
        integer(object, "limit");
        integer(object, "bad");
        boolean(object, "allow_large");
        boolean(object, "replace_all");
        assert_eq!(
            args,
            json!({
                "offset": 12,
                "limit": 3,
                "allow_large": true,
                "replace_all": false,
                "bad": "x",
            })
        );
    }
}
