#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrtFilter {
    Off = 0,
    Lottes = 1,
    Halo = 2,
}

impl CrtFilter {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => CrtFilter::Lottes,
            2 => CrtFilter::Halo,
            _ => CrtFilter::Off,
        }
    }

    #[allow(dead_code)]
    pub fn next(&self) -> Self {
        match self {
            CrtFilter::Off => CrtFilter::Lottes,
            CrtFilter::Lottes => CrtFilter::Off,
            CrtFilter::Halo => CrtFilter::Off,
        }
    }
}

impl std::fmt::Display for CrtFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            CrtFilter::Off => "Off",
            CrtFilter::Lottes => "Lottes (CRT)",
            CrtFilter::Halo => "Halo (CRT)",
        };
        write!(f, "{}", text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crt_filter_from_u8() {
        assert_eq!(CrtFilter::from_u8(0), CrtFilter::Off);
        assert_eq!(CrtFilter::from_u8(1), CrtFilter::Lottes);
        assert_eq!(CrtFilter::from_u8(2), CrtFilter::Halo);
        assert_eq!(CrtFilter::from_u8(3), CrtFilter::Off); // Default case
    }

    #[test]
    fn test_crt_filter_next() {
        assert_eq!(CrtFilter::Off.next(), CrtFilter::Lottes);
        assert_eq!(CrtFilter::Lottes.next(), CrtFilter::Off);
        assert_eq!(CrtFilter::Halo.next(), CrtFilter::Off);
    }

    #[test]
    fn test_crt_filter_to_string() {
        assert_eq!(CrtFilter::Off.to_string(), "Off");
        assert_eq!(CrtFilter::Lottes.to_string(), "Lottes (CRT)");
        assert_eq!(CrtFilter::Halo.to_string(), "Halo (CRT)");
    }
}
