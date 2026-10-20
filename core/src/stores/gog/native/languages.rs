//! GOG's language table (gogdl's `languages.py`): v2 depots name languages
//! by code (`en-US`), v1 depots by English name (`English`), and installs
//! made by other tools may use the deprecated short codes (`en`, `jp`).
//! Every form maps to the code we store.

pub struct Lang {
    pub code: &'static str,
    pub name: &'static str,
    pub deprecated: &'static [&'static str],
}

const LANGUAGES: &[Lang] = &[
    Lang {
        code: "af-ZA",
        name: "Afrikaans",
        deprecated: &[],
    },
    Lang {
        code: "ar",
        name: "Arabic",
        deprecated: &[],
    },
    Lang {
        code: "az-AZ",
        name: "Azeri",
        deprecated: &[],
    },
    Lang {
        code: "be-BY",
        name: "Belarusian",
        deprecated: &["be"],
    },
    Lang {
        code: "bn-BD",
        name: "Bengali",
        deprecated: &["bn_BD"],
    },
    Lang {
        code: "bg-BG",
        name: "Bulgarian",
        deprecated: &["bg", "bl"],
    },
    Lang {
        code: "bs-BA",
        name: "Bosnian",
        deprecated: &[],
    },
    Lang {
        code: "ca-ES",
        name: "Catalan",
        deprecated: &["ca"],
    },
    Lang {
        code: "cs-CZ",
        name: "Czech",
        deprecated: &["cz"],
    },
    Lang {
        code: "cy-GB",
        name: "Welsh",
        deprecated: &[],
    },
    Lang {
        code: "da-DK",
        name: "Danish",
        deprecated: &["da"],
    },
    Lang {
        code: "de-DE",
        name: "German",
        deprecated: &["de"],
    },
    Lang {
        code: "dv-MV",
        name: "Divehi",
        deprecated: &[],
    },
    Lang {
        code: "el-GR",
        name: "Greek",
        deprecated: &["gk", "el-GK"],
    },
    Lang {
        code: "en-GB",
        name: "British English",
        deprecated: &["en_GB"],
    },
    Lang {
        code: "en-US",
        name: "English",
        deprecated: &["en"],
    },
    Lang {
        code: "es-ES",
        name: "Spanish",
        deprecated: &["es"],
    },
    Lang {
        code: "es-MX",
        name: "Latin American Spanish",
        deprecated: &["es_mx"],
    },
    Lang {
        code: "et-EE",
        name: "Estonian",
        deprecated: &["et"],
    },
    Lang {
        code: "eu-ES",
        name: "Basque",
        deprecated: &[],
    },
    Lang {
        code: "fa-IR",
        name: "Persian",
        deprecated: &["fa"],
    },
    Lang {
        code: "fi-FI",
        name: "Finnish",
        deprecated: &["fi"],
    },
    Lang {
        code: "fo-FO",
        name: "Faroese",
        deprecated: &[],
    },
    Lang {
        code: "fr-FR",
        name: "French",
        deprecated: &["fr"],
    },
    Lang {
        code: "gl-ES",
        name: "Galician",
        deprecated: &[],
    },
    Lang {
        code: "gu-IN",
        name: "Gujarati",
        deprecated: &["gu"],
    },
    Lang {
        code: "he-IL",
        name: "Hebrew",
        deprecated: &["he"],
    },
    Lang {
        code: "hi-IN",
        name: "Hindi",
        deprecated: &["hi"],
    },
    Lang {
        code: "hr-HR",
        name: "Croatian",
        deprecated: &[],
    },
    Lang {
        code: "hu-HU",
        name: "Hungarian",
        deprecated: &["hu"],
    },
    Lang {
        code: "hy-AM",
        name: "Armenian",
        deprecated: &[],
    },
    Lang {
        code: "id-ID",
        name: "Indonesian",
        deprecated: &[],
    },
    Lang {
        code: "is-IS",
        name: "Icelandic",
        deprecated: &["is"],
    },
    Lang {
        code: "it-IT",
        name: "Italian",
        deprecated: &["it"],
    },
    Lang {
        code: "ja-JP",
        name: "Japanese",
        deprecated: &["jp"],
    },
    Lang {
        code: "jv-ID",
        name: "Javanese",
        deprecated: &["jv"],
    },
    Lang {
        code: "ka-GE",
        name: "Georgian",
        deprecated: &[],
    },
    Lang {
        code: "kk-KZ",
        name: "Kazakh",
        deprecated: &[],
    },
    Lang {
        code: "kn-IN",
        name: "Kannada",
        deprecated: &[],
    },
    Lang {
        code: "ko-KR",
        name: "Korean",
        deprecated: &["ko"],
    },
    Lang {
        code: "kok-IN",
        name: "Konkani",
        deprecated: &[],
    },
    Lang {
        code: "ky-KG",
        name: "Kyrgyz",
        deprecated: &[],
    },
    Lang {
        code: "la",
        name: "Latin",
        deprecated: &[],
    },
    Lang {
        code: "lt-LT",
        name: "Lithuanian",
        deprecated: &[],
    },
    Lang {
        code: "lv-LV",
        name: "Latvian",
        deprecated: &[],
    },
    Lang {
        code: "ml-IN",
        name: "Malayalam",
        deprecated: &["ml"],
    },
    Lang {
        code: "mi-NZ",
        name: "Maori",
        deprecated: &[],
    },
    Lang {
        code: "mk-MK",
        name: "Macedonian",
        deprecated: &[],
    },
    Lang {
        code: "mn-MN",
        name: "Mongolian",
        deprecated: &[],
    },
    Lang {
        code: "mr-IN",
        name: "Marathi",
        deprecated: &["mr"],
    },
    Lang {
        code: "ms-MY",
        name: "Malay",
        deprecated: &[],
    },
    Lang {
        code: "mt-MT",
        name: "Maltese",
        deprecated: &[],
    },
    Lang {
        code: "nb-NO",
        name: "Norwegian",
        deprecated: &["no"],
    },
    Lang {
        code: "nl-NL",
        name: "Dutch",
        deprecated: &["nl"],
    },
    Lang {
        code: "ns-ZA",
        name: "Northern Sotho",
        deprecated: &[],
    },
    Lang {
        code: "pa-IN",
        name: "Punjabi",
        deprecated: &[],
    },
    Lang {
        code: "pl-PL",
        name: "Polish",
        deprecated: &["pl"],
    },
    Lang {
        code: "ps-AR",
        name: "Pashto",
        deprecated: &[],
    },
    Lang {
        code: "pt-BR",
        name: "Portuguese (Brazilian)",
        deprecated: &["br"],
    },
    Lang {
        code: "pt-PT",
        name: "Portuguese",
        deprecated: &["pt"],
    },
    Lang {
        code: "ro-RO",
        name: "Romanian",
        deprecated: &["ro"],
    },
    Lang {
        code: "ru-RU",
        name: "Russian",
        deprecated: &["ru"],
    },
    Lang {
        code: "sa-IN",
        name: "Sanskrit",
        deprecated: &[],
    },
    Lang {
        code: "sk-SK",
        name: "Slovak",
        deprecated: &["sk"],
    },
    Lang {
        code: "sl-SI",
        name: "Slovenian",
        deprecated: &[],
    },
    Lang {
        code: "sq-AL",
        name: "Albanian",
        deprecated: &[],
    },
    Lang {
        code: "sr-SP",
        name: "Serbian",
        deprecated: &["sb"],
    },
    Lang {
        code: "sv-SE",
        name: "Swedish",
        deprecated: &["sv"],
    },
    Lang {
        code: "sw-KE",
        name: "Kiswahili",
        deprecated: &[],
    },
    Lang {
        code: "ta-IN",
        name: "Tamil",
        deprecated: &["ta_IN"],
    },
    Lang {
        code: "te-IN",
        name: "Telugu",
        deprecated: &["te"],
    },
    Lang {
        code: "th-TH",
        name: "Thai",
        deprecated: &["th"],
    },
    Lang {
        code: "tl-PH",
        name: "Tagalog",
        deprecated: &[],
    },
    Lang {
        code: "tn-ZA",
        name: "Setswana",
        deprecated: &[],
    },
    Lang {
        code: "tr-TR",
        name: "Turkish",
        deprecated: &["tr"],
    },
    Lang {
        code: "tt-RU",
        name: "Tatar",
        deprecated: &[],
    },
    Lang {
        code: "uk-UA",
        name: "Ukrainian",
        deprecated: &["uk"],
    },
    Lang {
        code: "ur-PK",
        name: "Urdu",
        deprecated: &["ur_PK"],
    },
    Lang {
        code: "uz-UZ",
        name: "Uzbek",
        deprecated: &[],
    },
    Lang {
        code: "vi-VN",
        name: "Vietnamese",
        deprecated: &["vi"],
    },
    Lang {
        code: "xh-ZA",
        name: "isiXhosa",
        deprecated: &[],
    },
    Lang {
        code: "zh-Hans",
        name: "Chinese (Simplified)",
        deprecated: &["zh_Hans", "zh", "cn"],
    },
    Lang {
        code: "zh-Hant",
        name: "Chinese (Traditional)",
        deprecated: &["zh_Hant"],
    },
    Lang {
        code: "zu-ZA",
        name: "isiZulu",
        deprecated: &[],
    },
];

/// The language named by a code, an English name or a deprecated code.
pub fn parse(value: &str) -> Option<&'static Lang> {
    let v = value.trim();
    LANGUAGES.iter().find(|l| {
        l.code.eq_ignore_ascii_case(v)
            || l.name.eq_ignore_ascii_case(v)
            || l.deprecated.iter().any(|d| d.eq_ignore_ascii_case(v))
    })
}

/// The code we store for any spelling; unknown ones are kept as given.
pub fn code(value: &str) -> String {
    parse(value)
        .map(|l| l.code.to_string())
        .unwrap_or_else(|| value.to_string())
}

/// Does a depot's language (`*` / `Neutral` = every language) cover the
/// install language `code`?
pub fn depot_matches(depot_lang: &str, code: &str) -> bool {
    depot_lang == "*"
        || depot_lang == "Neutral"
        || parse(depot_lang).is_some_and(|l| l.code.eq_ignore_ascii_case(code))
        || depot_lang.eq_ignore_ascii_case(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spelling_maps_to_the_code() {
        assert_eq!(code("English"), "en-US");
        assert_eq!(code("en"), "en-US");
        assert_eq!(code("en-us"), "en-US");
        assert_eq!(code("Ukrainian"), "uk-UA");
        assert_eq!(code("cn"), "zh-Hans");
        assert_eq!(code("klingon"), "klingon");
    }

    #[test]
    fn depots_match_by_name_or_code() {
        assert!(depot_matches("English", "en-US"));
        assert!(depot_matches("en-US", "en-US"));
        assert!(depot_matches("Neutral", "pl-PL"));
        assert!(depot_matches("*", "pl-PL"));
        assert!(!depot_matches("German", "en-US"));
    }
}
