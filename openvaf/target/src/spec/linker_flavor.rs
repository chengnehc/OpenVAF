// TODO(JW) Add lld?
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LinkerFlavor {
    Ld,   // GNU Linux
    Ld64, // Mac OSX
    Msvc, // Windows
}

macro_rules! flavor_mappings {
    ($ ((($( $flavor:tt )*), $string:expr),) *) => (
        impl LinkerFlavor {
            pub const fn one_of() -> &'static str {
                concat!("one of: ", $( $string, " ",) *)
            }

            #[allow(clippy::should_implement_trait)]
            pub fn from_str(s: &str) -> Option<Self> {
                Some(match s {
                    $( $string => $( $flavor )*, )*
                    _ => return None,
                })
            }

            pub fn desc(&self) -> &str {
                match *self {
                    $( $( $flavor )* => $string, )*
                }
            }
        }
    )
}

flavor_mappings! {
    ((LinkerFlavor::Ld), "ld"),
    ((LinkerFlavor::Ld64), "ld64"),
    ((LinkerFlavor::Msvc), "msvc"),
}
