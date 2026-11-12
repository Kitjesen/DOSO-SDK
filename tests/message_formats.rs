use doso_sdk::{BatteryState, CameraFrame, CameraInfo, CameraSample, MapGrid, Pose};
use prost::Message;

#[test]
fn unavailable_values_remain_distinct_from_zero() {
    let battery = BatteryState {
        percent: Some(0.0),
        charging: Some(false),
        ..Default::default()
    };
    let decoded = BatteryState::decode(battery.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.percent, Some(0.0));
    assert_eq!(decoded.charging, Some(false));
    assert_eq!(decoded.voltage_v, None);
}

#[test]
fn media_and_grid_preserve_payloads_and_metadata() {
    let image = CameraSample {
        frame: Some(CameraFrame { jpeg: vec![1, 2] }),
        info: Some(CameraInfo {
            width_px: Some(640),
            height_px: Some(480),
            ..Default::default()
        }),
        sequence: Some(7),
        ..Default::default()
    };
    let decoded = CameraSample::decode(image.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.frame.unwrap().jpeg, [1, 2]);
    assert_eq!(decoded.info.unwrap().width_px, Some(640));
    assert_eq!(decoded.sequence, Some(7));

    let grid = MapGrid {
        width: 2,
        height: 1,
        resolution_m: 0.05,
        origin: Some(Pose::default()),
        cells: vec![0, 255],
        ..Default::default()
    };
    let decoded = MapGrid::decode(grid.encode_to_vec().as_slice()).unwrap();
    assert_eq!((decoded.width, decoded.height), (2, 1));
    assert_eq!(decoded.cells, [0, 255]);
}
