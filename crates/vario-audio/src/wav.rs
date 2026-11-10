//! Minimal WAV file writer: mono, 16 bit.

use std::io::{self, Write};

/// Writes samples in the range -1.0..=1.0 as a WAV file to `writer`.
///
/// `impl Write` accepts anything bytes can be written to: a file, a `Vec<u8>`, a
/// network connection.
pub fn write_wav(mut writer: impl Write, sample_rate_hz: u32, samples: &[f32]) -> io::Result<()> {
    const BYTES_PER_SAMPLE: u32 = 2;
    let data_len = u32::try_from(samples.len())
        .ok()
        .and_then(|count| count.checked_mul(BYTES_PER_SAMPLE))
        .filter(|len| *len <= u32::MAX - 36)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "too many samples for WAV"))?;

    // All numbers in a WAV header are little endian.
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36 + data_len).to_le_bytes())?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&16u32.to_le_bytes())?; // length of the format block
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&1u16.to_le_bytes())?; // channels
    writer.write_all(&sample_rate_hz.to_le_bytes())?;
    writer.write_all(&(sample_rate_hz * BYTES_PER_SAMPLE).to_le_bytes())?; // bytes per second
    writer.write_all(&(BYTES_PER_SAMPLE as u16).to_le_bytes())?; // bytes per frame
    writer.write_all(&16u16.to_le_bytes())?; // bits per sample
    writer.write_all(b"data")?;
    writer.write_all(&data_len.to_le_bytes())?;

    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_header_and_samples() {
        let mut bytes = Vec::new();
        write_wav(&mut bytes, 48_000, &[0.0, 1.0, -1.0, 2.0]).unwrap();

        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(bytes[4..8], 44u32.to_le_bytes());
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(bytes[24..28], 48_000u32.to_le_bytes());
        assert_eq!(bytes[28..32], 96_000u32.to_le_bytes());
        assert_eq!(bytes[40..44], 8u32.to_le_bytes());
        assert_eq!(bytes[44..46], 0i16.to_le_bytes());
        assert_eq!(bytes[46..48], i16::MAX.to_le_bytes());
        assert_eq!(bytes[48..50], (-i16::MAX).to_le_bytes());
        assert_eq!(bytes[50..52], i16::MAX.to_le_bytes(), "clipped");
    }
}
