use bytes::BytesMut;
use serde_json::Value;
use std::error::Error;
use tokio_postgres::types::{Format, FromSql, IsNull, Kind, ToSql, Type, to_sql_checked};

type BoxError = Box<dyn Error + Sync + Send>;

/// Bind in the PostgreSQL text format; the server converts to its inferred type.
/// Values are protocol parameters, never interpolated into SQL.
#[derive(Debug)]
pub(super) struct Parameter(pub Option<String>);
impl Parameter {
    pub fn new(value: Value) -> Self {
        Self(match value {
            Value::Null => None,
            Value::String(value) => Some(value),
            value => Some(value.to_string()),
        })
    }
    pub fn len(&self) -> usize {
        self.0.as_ref().map_or(0, String::len)
    }
}
impl ToSql for Parameter {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        match &self.0 {
            None => Ok(IsNull::Yes),
            Some(value) => {
                out.extend_from_slice(value.as_bytes());
                Ok(IsNull::No)
            }
        }
    }
    fn accepts(_: &Type) -> bool {
        true
    }
    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }
    to_sql_checked!();
}

#[derive(Debug)]
pub(super) struct Cell(pub Value);
impl<'a> FromSql<'a> for Cell {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        if let Kind::Domain(base) = ty.kind() {
            return Self::from_sql(base, raw);
        }
        let value = match *ty {
            Type::BOOL => Value::Bool(bool::from_sql(ty, raw)?),
            Type::CHAR => Value::from(i8::from_sql(ty, raw)?),
            Type::INT2 => Value::from(i16::from_sql(ty, raw)?),
            Type::INT4 => Value::from(i32::from_sql(ty, raw)?),
            Type::INT8 => Value::from(i64::from_sql(ty, raw)?),
            Type::OID => Value::from(u32::from_sql(ty, raw)?),
            Type::FLOAT4 => float(f32::from_sql(ty, raw)? as f64),
            Type::FLOAT8 => float(f64::from_sql(ty, raw)?),
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
                Value::String(std::str::from_utf8(raw)?.into())
            }
            Type::JSON | Type::JSONB => Value::from_sql(ty, raw)?,
            Type::UUID => Value::String(uuid::Uuid::from_sql(ty, raw)?.to_string()),
            Type::DATE => Value::String(chrono::NaiveDate::from_sql(ty, raw)?.to_string()),
            Type::TIME => Value::String(chrono::NaiveTime::from_sql(ty, raw)?.to_string()),
            Type::TIMESTAMP => Value::String(
                chrono::NaiveDateTime::from_sql(ty, raw)?
                    .format("%Y-%m-%dT%H:%M:%S%.f")
                    .to_string(),
            ),
            Type::TIMESTAMPTZ => {
                Value::String(chrono::DateTime::<chrono::Utc>::from_sql(ty, raw)?.to_rfc3339())
            }
            Type::BYTEA => Value::String(format!(
                "\\x{}",
                raw.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )),
            Type::NUMERIC => Value::String(numeric(raw)?),
            _ => match ty.kind() {
                Kind::Enum(_) => Value::String(std::str::from_utf8(raw)?.into()),
                Kind::Array(_) => Value::Array(
                    Vec::<Cell>::from_sql(ty, raw)?
                        .into_iter()
                        .map(|cell| cell.0)
                        .collect(),
                ),
                _ => {
                    return Err(format!(
                        "unsupported PostgreSQL result type {}; cast the column to text",
                        ty.name()
                    )
                    .into());
                }
            },
        };
        Ok(Self(value))
    }
    fn from_sql_null(_: &Type) -> Result<Self, BoxError> {
        Ok(Self(Value::Null))
    }
    fn accepts(_: &Type) -> bool {
        true
    }
}
fn float(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map_or_else(|| Value::String(value.to_string()), Value::Number)
}

// PostgreSQL NUMERIC's base-10000 binary representation. Preserve decimal scale
// as a JSON string instead of rounding money/high-precision numbers to f64.
fn numeric(raw: &[u8]) -> Result<String, BoxError> {
    if raw.len() < 8 || !raw.len().is_multiple_of(2) {
        return Err("invalid NUMERIC value".into());
    }
    let word = |index| u16::from_be_bytes([raw[index], raw[index + 1]]);
    let count = word(0) as usize;
    if raw.len() != 8 + 2 * count {
        return Err("invalid NUMERIC length".into());
    }
    let weight = word(2) as i16 as i32;
    let sign = word(4);
    let scale = word(6) as usize;
    match sign {
        0xC000 => return Ok("NaN".into()),
        0xD000 => return Ok("Infinity".into()),
        0xF000 => return Ok("-Infinity".into()),
        0 | 0x4000 => {}
        _ => return Err("invalid NUMERIC sign".into()),
    }
    let digits = (0..count).map(|i| word(8 + i * 2)).collect::<Vec<_>>();
    if digits.iter().any(|n| *n >= 10000) {
        return Err("invalid NUMERIC digit".into());
    }
    let digit = |position: i32| {
        digits
            .get((weight - position) as usize)
            .copied()
            .unwrap_or(0)
    };
    let mut output = String::new();
    if sign == 0x4000 && digits.iter().any(|n| *n != 0) {
        output.push('-');
    }
    if weight < 0 {
        output.push('0');
    } else {
        for pos in (0..=weight).rev() {
            if pos == weight {
                output.push_str(&digit(pos).to_string());
            } else {
                output.push_str(&format!("{:04}", digit(pos)));
            }
        }
    }
    if scale > 0 {
        output.push('.');
        let mut fraction = String::new();
        for i in 1..=scale.div_ceil(4) {
            fraction.push_str(&format!("{:04}", digit(-(i as i32))));
        }
        output.push_str(&fraction[..scale]);
    }
    Ok(output)
}
