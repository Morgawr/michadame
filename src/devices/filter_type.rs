#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrtFilter {
    Off = 0,
    Lottes = 1,
}

impl CrtFilter {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => CrtFilter::Lottes,
            _ => CrtFilter::Off,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            CrtFilter::Off => CrtFilter::Lottes,
            CrtFilter::Lottes => CrtFilter::Off,
        }
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            CrtFilter::Off => "Off",
            CrtFilter::Lottes => "Lottes (CRT)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crt_filter_from_u8() {
        assert_eq!(CrtFilter::from_u8(0), CrtFilter::Off);
        assert_eq!(CrtFilter::from_u8(1), CrtFilter::Lottes);
        assert_eq!(CrtFilter::from_u8(2), CrtFilter::Off); // Default case
    }

    #[test]
    fn test_crt_filter_next() {
        assert_eq!(CrtFilter::Off.next(), CrtFilter::Lottes);
        assert_eq!(CrtFilter::Lottes.next(), CrtFilter::Off);
    }

    #[test]
    fn test_crt_filter_to_string() {
        assert_eq!(CrtFilter::Off.to_string(), "Off");
        assert_eq!(CrtFilter::Lottes.to_string(), "Lottes (CRT)");
    }
}
