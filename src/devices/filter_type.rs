#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrtFilter {
    Off = 0,
    Scanlines = 1,
    Lottes = 2,
}

impl CrtFilter {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => CrtFilter::Scanlines,
            2 => CrtFilter::Lottes,
            _ => CrtFilter::Off,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            CrtFilter::Off => CrtFilter::Scanlines,
            CrtFilter::Scanlines => CrtFilter::Lottes,
            CrtFilter::Lottes => CrtFilter::Off,
        }
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            CrtFilter::Off => "Off",
            CrtFilter::Scanlines => "Scanlines",
            CrtFilter::Lottes => "Lottes (Advanced)",
        }
    }
}