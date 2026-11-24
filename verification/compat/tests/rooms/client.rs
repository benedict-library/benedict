use super::Library;
use serde_json::{Value, json};

#[test]
fn settings_prefer_the_environment_over_the_settings_file() {
    let mut l = Library::new();
    l.write(
        "search.env",
        "LIBRARY_REGISTRY_ROOM=file-value\nLIBRARY_MAIL_ROOM=chosen-room\n",
    );
    l.env.remove("LIBRARY_MAIL_ROOM");
    let probe = "import common,json; d=common.settings(); print(json.dumps([d['LIBRARY_REGISTRY_ROOM'],d['LIBRARY_MAIL_ROOM']]))";
    let settings: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(settings, json!(["main-search", "chosen-room"]));
}

#[test]
fn room_calls_apply_a_local_timeout_or_bound_the_remote_call_and_preserve_quoting() {
    let l = Library::new();
    let probe = r#"import json, common
from unittest.mock import patch
cfg={'LIBRARY_ROOM_CMD':'python room.py exec'}
with patch.object(common,'run',return_value=(0,'','')) as called:
 common.room_exec(cfg,'study','pass',7,9)
 common.room_exec(dict(cfg, LIBRARY_ROOM_EXEC='transport host'),'study','pass',7,9)
 print(json.dumps([c.args for c in called.call_args_list]))
"#;
    let calls: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(
        calls[0],
        json!([["python", "room.py", "exec", "study"], "pass", 7])
    );
    assert_eq!(
        calls[1],
        json!([
            ["transport", "host", "timeout 7 python room.py exec study"],
            "pass",
            9
        ])
    );
    let probe = "import common,json,shlex; parts=['python','-c',\"print('a b')\"]; routed=common.on('transport host',parts); print(json.dumps([routed[:2],shlex.split(routed[-1]),common.on('',parts)]))";
    let words: Value = serde_json::from_str(&l.python(&["-c", probe], "").ok()).unwrap();
    assert_eq!(
        words,
        json!([
            ["transport", "host"],
            ["python", "-c", "print('a b')"],
            ["python", "-c", "print('a b')"]
        ])
    );
}
