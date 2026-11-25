use crate::{rpc, DosoClient, Error, Result};

use super::Observations;

const MAX_FRAME_BYTES: usize = 10 * 1024 * 1024;
const MAX_POINTS: usize = 1_000_000;
const HEADER_BYTES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudStreamKind {
    Cloud,
    Map,
    Scan,
    Reset,
}

#[derive(Debug)]
pub struct PointCloud {
    pub points: Vec<[f32; 3]>,
    pub colors: Option<Vec<[u8; 3]>>,
    pub frame_id: String,
    pub epoch: u32,
    pub sequence: u32,
    pub stamp_s: f64,
    pub stream_kind: CloudStreamKind,
    pub scale_m: f32,
}

impl Observations<'_> {
    /// Receive viewer point-cloud frames until the stream is dropped.
    pub async fn watch_point_cloud(
        &self,
        kind: rpc::CloudKind,
    ) -> Result<tonic::Streaming<rpc::CloudFrame>> {
        if kind == rpc::CloudKind::Unspecified {
            return Err(Error::invalid("point-cloud kind must be Map or Scan"));
        }
        DosoClient::read(
            rpc::observations_client::ObservationsClient::new(self.0.channel.clone())
                .max_decoding_message_size(MAX_FRAME_BYTES + 1024)
                .watch_point_cloud(
                    self.0
                        .stream_request(rpc::CloudRequest { kind: kind as i32 }),
                )
                .await,
        )
    }
}

impl rpc::CloudFrame {
    /// Decode the existing PCLD v2 viewer format into metre coordinates and RGB.
    pub fn decode(&self) -> Result<PointCloud> {
        let data = &self.pcld;
        if data.len() < HEADER_BYTES || data.len() > MAX_FRAME_BYTES {
            return Err(Error::protocol("invalid point-cloud frame length"));
        }
        if &data[..4] != b"PCLD" || data[4] != 2 {
            return Err(Error::protocol(
                "point-cloud decoder requires PCLD version 2",
            ));
        }
        if data[5] & !1 != 0 {
            return Err(Error::protocol("unsupported point-cloud flags"));
        }
        let read_u32 = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        let read_f32 = |offset| f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        let header_size = usize::from(u16::from_le_bytes(data[6..8].try_into().unwrap()));
        let count = read_u32(8) as usize;
        let scale = read_f32(12);
        let origin = [read_f32(16), read_f32(20), read_f32(24)];
        let stamp_s = f64::from_le_bytes(data[36..44].try_into().unwrap());
        if !scale.is_finite()
            || scale <= 0.0
            || !origin.iter().all(|value| value.is_finite())
            || !stamp_s.is_finite()
        {
            return Err(Error::protocol(
                "point-cloud scale, origin or time is invalid",
            ));
        }
        let frame_end = HEADER_BYTES + usize::from(data[45]);
        if header_size < frame_end || header_size > data.len() || count > MAX_POINTS {
            return Err(Error::protocol("invalid point-cloud header or point count"));
        }
        let point_bytes = count * 6;
        let color_bytes = if data[5] & 1 != 0 { count * 3 } else { 0 };
        if header_size + point_bytes + color_bytes != data.len() {
            return Err(Error::protocol(
                "point-cloud payload length does not match its header",
            ));
        }
        let frame_id = std::str::from_utf8(&data[HEADER_BYTES..frame_end])
            .map_err(|_| Error::protocol("point-cloud frame ID is not UTF-8"))?
            .to_owned();
        let stream_kind = match data[44] {
            0 => CloudStreamKind::Cloud,
            1 => CloudStreamKind::Map,
            2 => CloudStreamKind::Scan,
            3 => CloudStreamKind::Reset,
            _ => return Err(Error::protocol("unsupported point-cloud stream kind")),
        };
        let positions = &data[header_size..header_size + point_bytes];
        let points = positions
            .chunks_exact(6)
            .map(|point| {
                let axis = |index| {
                    f32::from(i16::from_le_bytes([point[index * 2], point[index * 2 + 1]])) * scale
                        + origin[index]
                };
                [axis(0), axis(1), axis(2)]
            })
            .collect();
        let colors = (data[5] & 1 != 0).then(|| {
            data[header_size + point_bytes..]
                .chunks_exact(3)
                .map(|color| [color[0], color[1], color[2]])
                .collect()
        });
        Ok(PointCloud {
            points,
            colors,
            frame_id,
            epoch: read_u32(28),
            sequence: read_u32(32),
            stamp_s,
            stream_kind,
            scale_m: scale,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: &[u8] = include_bytes!("../../tests/fixtures/cloud-v2.bin");

    #[test]
    fn python_frame_preserves_coordinates_and_metadata() {
        let frame = rpc::CloudFrame {
            pcld: FRAME.to_vec(),
        }
        .decode()
        .unwrap();
        assert_eq!(frame.points, [[1.0, 2.0, 3.0], [-1.0, -2.0, -3.0]]);
        assert_eq!(frame.colors.unwrap(), [[255, 0, 1], [2, 3, 4]]);
        assert_eq!(
            (frame.frame_id.as_str(), frame.epoch, frame.sequence),
            ("map", 7, 9)
        );
        assert_eq!(frame.stamp_s, 123.25);
        assert_eq!(frame.stream_kind, CloudStreamKind::Map);
        assert_eq!(frame.scale_m, 0.005);
    }

    #[test]
    fn python_reset_preserves_empty_scene() {
        let frame = rpc::CloudFrame {
            pcld: include_bytes!("../../tests/fixtures/cloud-reset.bin").to_vec(),
        }
        .decode()
        .unwrap();
        assert!(frame.points.is_empty());
        assert!(frame.colors.is_none());
        assert_eq!((frame.epoch, frame.sequence, frame.stamp_s), (8, 10, 124.5));
        assert_eq!(frame.stream_kind, CloudStreamKind::Reset);
    }

    #[test]
    fn rejects_malformed_frames() {
        let mut cases = vec![vec![], FRAME[..FRAME.len() - 1].to_vec()];
        for (offset, value) in [(0, 0), (4, 1), (5, 2), (44, 4), (45, 255), (48, 255)] {
            let mut frame = FRAME.to_vec();
            frame[offset] = value;
            cases.push(frame);
        }
        for (offset, value) in [
            (12, f32::NAN.to_le_bytes()),
            (16, f32::INFINITY.to_le_bytes()),
            (8, 1_000_001_u32.to_le_bytes()),
        ] {
            let mut frame = FRAME.to_vec();
            frame[offset..offset + 4].copy_from_slice(&value);
            cases.push(frame);
        }
        for pcld in cases {
            assert!(rpc::CloudFrame { pcld }.decode().is_err());
        }
    }
}
