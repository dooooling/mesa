//! 公共828D候选目录的地址与值回归，不把软件控制器回放当作硬件支持。
//! 请求字节来自已审计的LX路径及独立只读记录；值向量固定，避免回环生成器掩盖字节序错误。

use mesa_core_types::{AcquisitionTask, DataType, Value};
use mesa_driver_sdk::DriverConnection;
use mesa_driver_sinumerik_nck::{
    CatalogRegistry, NckConnConfig, NckConnection, NckDataKind, NckVariableRef, decode_value,
    encode_var_spec, resolve_wire,
};
use mesa_s7_transport::read_var::{S7ReadVarItem, parse_read_response};
use serde_json::json;

fn catalog() -> mesa_driver_sinumerik_nck::NckCatalog {
    CatalogRegistry::load_dir(&format!("{}/catalog", env!("CARGO_MANIFEST_DIR")))
        .unwrap()
        .resolve("828d")
        .unwrap()
}

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|s| u8::from_str_radix(s, 16).unwrap())
        .collect()
}

#[test]
fn public_mapping_requests_match_reviewed_wire_vectors() {
    let cases = [
        (
            "C",
            "S",
            "progStatus",
            1,
            1,
            "12 08 82 41 00 0d 00 01 7f 01",
        ),
        ("B", "S", "opMode", 1, 1, "12 08 82 21 00 03 00 01 7f 01"),
        (
            "C",
            "SPARPP",
            "progName",
            1,
            1,
            "12 08 82 41 00 05 00 01 7a 01",
        ),
        (
            "C",
            "SPARP",
            "progName",
            1,
            1,
            "12 08 82 41 00 03 00 01 7d 01",
        ),
        (
            "C",
            "SPARP",
            "actLineNumber",
            1,
            1,
            "12 08 82 41 00 09 00 01 7d 01",
        ),
        (
            "C",
            "SPARP",
            "actBlock",
            1,
            1,
            "12 08 82 41 00 1f 00 01 7d 01",
        ),
        (
            "C",
            "SGA",
            "actToolBasePos",
            1,
            3,
            "12 08 82 41 00 02 00 03 73 01",
        ),
        (
            "C",
            "SEGA",
            "actProgPosBKS",
            1,
            3,
            "12 08 82 41 00 21 00 03 70 01",
        ),
        (
            "C",
            "S",
            "actFeedRateIpo",
            1,
            1,
            "12 08 82 41 00 01 00 01 7f 01",
        ),
        (
            "C",
            "S",
            "cmdFeedRateIpo",
            1,
            1,
            "12 08 82 41 00 02 00 01 7f 01",
        ),
        (
            "C",
            "S",
            "feedRateIpoOvr",
            1,
            1,
            "12 08 82 41 00 03 00 01 7f 01",
        ),
        (
            "C",
            "SSP",
            "actSpeed",
            1,
            2,
            "12 08 82 41 00 02 00 02 72 01",
        ),
        (
            "C",
            "SSP",
            "cmdSpeed",
            1,
            2,
            "12 08 82 41 00 03 00 02 72 01",
        ),
        (
            "C",
            "SSP",
            "speedOvr",
            1,
            2,
            "12 08 82 41 00 04 00 02 72 01",
        ),
        (
            "C",
            "S",
            "actTNumber",
            1,
            1,
            "12 08 82 41 00 17 00 01 7f 01",
        ),
        ("C", "RP", "rpa", 1, 426, "12 08 82 41 00 01 01 aa 15 01"),
        ("V", "M", "r0027", 1, 1, "12 08 82 a1 00 1b 00 01 1a 01"),
        ("V", "M", "r0035", 3, 1, "12 08 82 a3 00 23 00 01 1a 01"),
    ];
    let catalog = catalog();
    for (area, block, variable, unit, line, expected) in cases {
        let reference = NckVariableRef::from_parameters(
            json!({
                "area":area,"area_no":unit,"block":block,"variable":variable,
                "line":line,"count":1,"unit_mode":"current",
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        let definition = catalog.lookup(reference.area, block, variable).unwrap();
        let resolved = resolve_wire(&reference, definition).unwrap();
        assert_eq!(
            encode_var_spec(&resolved.wire),
            bytes(expected),
            "{area}/{block}/{variable}"
        );
    }
}

#[test]
fn recorded_nc_response_and_signed_sentinels_decode_honestly() {
    // 独立诊断的程序状态响应：UWORD=5，transport=9，2字节小端。
    let frame =
        bytes("03 00 00 1b 02 f0 80 32 03 00 00 00 02 00 02 00 06 00 00 04 01 ff 09 00 02 05 00");
    let item = S7ReadVarItem {
        var_spec: vec![],
        expected_data_len: 2,
    };
    let result = parse_read_response(&frame, &[item]).unwrap();
    assert_eq!(result[0].return_code, 255);
    assert_eq!(
        decode_value(&result[0].data, NckDataKind::U16Le).unwrap(),
        Value::U32(5)
    );
    assert_eq!(
        decode_value(&bytes("fe ff ff ff"), NckDataKind::I32Le).unwrap(),
        Value::I32(-2)
    );
    assert_eq!(
        decode_value(&bytes("00 00 00 00 00 00 59 40"), NckDataKind::F64Le).unwrap(),
        Value::F64(100.0)
    );
    assert_eq!(
        decode_value(&bytes("41 c8 00 00"), NckDataKind::F32).unwrap(),
        Value::F32(25.0)
    );
    // 单行程序名不能把下一层子程序投影成主程序；Configure映射使用count=1。
    let mut program = [0; 32];
    program[..7].copy_from_slice(b"_N_MPF0");
    assert_eq!(
        decode_value(&program, NckDataKind::Utf8NulLast32).unwrap(),
        Value::String("_N_MPF0".into())
    );
}

#[tokio::test]
async fn public_catalog_configures_all_definitions_and_unknowns_still_fail_closed() {
    let catalog = catalog();
    let config = NckConnConfig::from_json(&json!({
        "host":"127.0.0.1","family":"828d","local_tsap":256,"remote_tsap":258,
    }))
    .unwrap();
    let connection = NckConnection::with_catalog(config, catalog.clone());
    let mut selections: Vec<_> = catalog.variables().into_iter().map(|definition| json!({
        "resource_id":"variable", "parameters":{
            "area":definition.area.letter().to_string(),"area_no":1,
            "block":definition.block,"variable":definition.variable,"line":1,"count":1,
        },"outputs":[{"output":"value","point_key":format!("{}/{}/{}",definition.area.letter(),definition.block,definition.variable)}],
    })).collect();
    // 最大原表R号及数组类型同时参与完整Configure，而不是仅查表。
    selections.push(json!({"resource_id":"variable","parameters":{
        "area":"C","area_no":1,"block":"RP","variable":"rpa","line":426,"count":3,
    },"outputs":[{"output":"value","point_key":"r425_427"}]}));
    let task: AcquisitionTask = serde_json::from_value(json!({
        "id":"public-mappings","schedule":{"mode":"poll","interval_ms":5000},
        "binding":{"kind":"mesa.resources.v1","config":{"selections":selections}},
    }))
    .unwrap();
    let descriptors = connection.configure(1, vec![task.clone()]).await.unwrap();
    assert_eq!(descriptors.len(), 43);
    assert_eq!(descriptors.last().unwrap().data_type, DataType::F64Array);
    assert_eq!(
        descriptors
            .iter()
            .filter(|d| d.data_type == DataType::String)
            .count(),
        5
    );
    let mut invalid = task;
    invalid.binding.config["selections"][0]["parameters"]["variable"] = json!("unknown_pending");
    let error = connection.configure(2, vec![invalid]).await.unwrap_err();
    assert_eq!(error.code, "INVALID_VARIABLE");
}
