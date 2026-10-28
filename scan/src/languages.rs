//! Tesseract's languages that Chat with Work Desktop can add by itself on
//! Linux: tessdata_fast's models at one commit, each with its size and
//! SHA-256, checked before a download is kept. Generated from
//! <https://github.com/tesseract-ocr/tessdata_fast/tree/87416418657359cb625c412a48b6e1d6d41c29bd>
//! (Apache-2.0). `osd` and `equ` are left out, since they aren't languages,
//! and `frk`, which is only a link to `deu_latf` there.

/// A language pack: Tesseract's code, its name in English, and the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pack {
    pub code: &'static str,
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// The commit every pack is downloaded from.
pub const COMMIT: &str = "87416418657359cb625c412a48b6e1d6d41c29bd";

impl Pack {
    /// Where this pack is downloaded from.
    pub fn url(&self) -> String {
        format!(
            "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/{COMMIT}/{}.traineddata",
            self.code
        )
    }
}

/// Every pack, by code.
pub fn pack(code: &str) -> Option<&'static Pack> {
    PACKS.iter().find(|p| p.code == code)
}

pub const PACKS: &[Pack] = &[
    Pack {
        code: "afr",
        name: "Afrikaans",
        bytes: 2652786,
        sha256: "126d480bfae95be2a911ed4916465e27bde75fea2da631e21b96762e5f239646",
    },
    Pack {
        code: "amh",
        name: "Amharic",
        bytes: 5470094,
        sha256: "3ec3311833a108e07d58a1152b00c0cf1848752e4f85769d46e8ca2b718a2ccc",
    },
    Pack {
        code: "ara",
        name: "Arabic",
        bytes: 1432056,
        sha256: "e3206d3dc87fd50c24a0fb9f01838615911d25168f4e64415244b67d2bb3e729",
    },
    Pack {
        code: "asm",
        name: "Assamese",
        bytes: 2045427,
        sha256: "299c7f6135ac72ca4820d4c39e3cf65b32b24127fc78c4938d11153adbb9fa77",
    },
    Pack {
        code: "aze",
        name: "Azerbaijani",
        bytes: 3524799,
        sha256: "a365310848aecb739f19369cb3831d4660fcd9345d798e91a3042455f9ccc9f0",
    },
    Pack {
        code: "aze_cyrl",
        name: "Azerbaijani (Cyrillic)",
        bytes: 1943672,
        sha256: "2e96ab67817819e14a3a2d6250261130457c8e78dde3077841ab391fe003551c",
    },
    Pack {
        code: "bel",
        name: "Belarusian",
        bytes: 3692098,
        sha256: "9c6668a0b202f3dcfe074b64620d108e1902ca7498a40b5a11b4a3da6112d58f",
    },
    Pack {
        code: "ben",
        name: "Bengali",
        bytes: 855841,
        sha256: "31163084c279aaebd376216f0c3d5c17ad4b5fee8db49dae79c20000b5de5964",
    },
    Pack {
        code: "bod",
        name: "Tibetan",
        bytes: 1966440,
        sha256: "1aab1db8dae337dfc8fa0f94a1c137c47e4c7857c3e6b30093ec726cef912a0b",
    },
    Pack {
        code: "bos",
        name: "Bosnian",
        bytes: 2498242,
        sha256: "6cc8cc87cf1afbfa6a41febb725dbadb14bed96a46685906440a6eb8a7892f04",
    },
    Pack {
        code: "bre",
        name: "Breton",
        bytes: 6335007,
        sha256: "db8b31f76b986d63b82dfff9e84e252bf5f7dc53d9128167aaab18b3ec96dd49",
    },
    Pack {
        code: "bul",
        name: "Bulgarian",
        bytes: 1675212,
        sha256: "aebc9b0fcc8cfaf8a9f38a02bb7b85052bd850744696a2c11cf0081820e5b21e",
    },
    Pack {
        code: "cat",
        name: "Catalan",
        bytes: 1146012,
        sha256: "250db73cd5b380d2798581295dc12f20d0828cdb335a65d833d12dfdbf57117d",
    },
    Pack {
        code: "ceb",
        name: "Cebuano",
        bytes: 716214,
        sha256: "346a39de801495c93ece6656d018d7d19d298af7db42d82ebae99dfe8ee14961",
    },
    Pack {
        code: "ces",
        name: "Czech",
        bytes: 3795684,
        sha256: "934bcaf97ef3348413263331131c9fa7f55f30db333c711929c124fb635f7e1b",
    },
    Pack {
        code: "chi_sim",
        name: "Chinese (Simplified)",
        bytes: 2469156,
        sha256: "a5fcb6f0db1e1d6d8522f39db4e848f05984669172e584e8d76b6b3141e1f730",
    },
    Pack {
        code: "chi_sim_vert",
        name: "Chinese (Simplified, vertical)",
        bytes: 1927902,
        sha256: "20590de84725bab69cde93bd6e8ed360a13cc5421a7e7364ddeb93e9af53d6da",
    },
    Pack {
        code: "chi_tra",
        name: "Chinese (Traditional)",
        bytes: 2366642,
        sha256: "529c5b5797d64b126065cd55f2bb4c7fd7b15790798091b1ff259941a829330b",
    },
    Pack {
        code: "chi_tra_vert",
        name: "Chinese (Traditional, vertical)",
        bytes: 1824756,
        sha256: "1df02a4b210e5c217b783819538b63e9dfe6904e2b5e53b62664f1b9f7a989d0",
    },
    Pack {
        code: "chr",
        name: "Cherokee",
        bytes: 374944,
        sha256: "a4da60d844ebb9dbaeb8843a92290083c1fd275b167fc9c9686d790ccd311fab",
    },
    Pack {
        code: "cos",
        name: "Corsican",
        bytes: 2299099,
        sha256: "018952d1b4b3d4fd6ee5ebbacf90603c9cef6221951359937c274fe1f190f8d3",
    },
    Pack {
        code: "cym",
        name: "Welsh",
        bytes: 2208919,
        sha256: "7f6ee3374749645a7c92dfe773f5c3d6492194d371712ecfd775edc53c363fb4",
    },
    Pack {
        code: "dan",
        name: "Danish",
        bytes: 2580059,
        sha256: "acb1fd074487a31d1294fcdfd7d7c673467ffd8aeacb2ccd61ebcbf04eb4e2fa",
    },
    Pack {
        code: "deu",
        name: "German",
        bytes: 1525436,
        sha256: "19d219bbb6672c869d20a9636c6816a81eb9a71796cb93ebe0cb1530e2cdb22d",
    },
    Pack {
        code: "deu_latf",
        name: "German (Fraktur)",
        bytes: 6423052,
        sha256: "7cd1b541e9d3884b9546a7d292c4f349cdb45ff646b4b43166dfc5099a8ad1a1",
    },
    Pack {
        code: "div",
        name: "Dhivehi",
        bytes: 1774535,
        sha256: "06051412588963b34b663cc59a468dbf10753329f015ccd574ee711e8c6b84c3",
    },
    Pack {
        code: "dzo",
        name: "Dzongkha",
        bytes: 449596,
        sha256: "364d244fad3f8d4438c9f7ee7642b9b876a4620cf76c0115619b979d6bfef1fa",
    },
    Pack {
        code: "ell",
        name: "Greek",
        bytes: 1419514,
        sha256: "4fba8a0b461038d51f1c20d043d4f2ac38c4e778f1b90830847f7bd8fa3ba726",
    },
    Pack {
        code: "eng",
        name: "English",
        bytes: 4113088,
        sha256: "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2",
    },
    Pack {
        code: "enm",
        name: "English (Middle, 1100-1500)",
        bytes: 3101551,
        sha256: "d312ab5aafc9f99a78082c75e035b0873d2da6cb6d95cde03e219c1b26750034",
    },
    Pack {
        code: "epo",
        name: "Esperanto",
        bytes: 4728393,
        sha256: "71181a6a07af3812aeedfa1aa993623424f4b8a6aac3e271b36ec11774e674d9",
    },
    Pack {
        code: "est",
        name: "Estonian",
        bytes: 4458101,
        sha256: "515d4a773682b286369511e83fe412bcff16a92a886f99c761c1d760a7e30456",
    },
    Pack {
        code: "eus",
        name: "Basque",
        bytes: 5175921,
        sha256: "40e7418296c355d9fd9ca843d115c51e740089e576e887443e61704823cd6624",
    },
    Pack {
        code: "fao",
        name: "Faroese",
        bytes: 3439772,
        sha256: "0ed46203c283c952b7de0530d1c6e6ac9a25a79773fb89cadd149275ffea9a80",
    },
    Pack {
        code: "fas",
        name: "Persian",
        bytes: 431500,
        sha256: "db1c0a91208aff00d3cf1ed2c1d23f76419afd5f024688b4f71adc3f2ce4a505",
    },
    Pack {
        code: "fil",
        name: "Filipino",
        bytes: 1841715,
        sha256: "2b17f68014ab44b88b5e2334ebcd0ce0090d9af7c6fbef6fab22e0bbd9a934dd",
    },
    Pack {
        code: "fin",
        name: "Finnish",
        bytes: 7865732,
        sha256: "61a04cd62b507c3d9ae0e1cda399e6715ebf49dea9df47897c8acdcd3bd3e13c",
    },
    Pack {
        code: "fra",
        name: "French",
        bytes: 1130365,
        sha256: "ced037562e8c80c13122dece28dd477d399af80911a28791a66a63ac1e3445ca",
    },
    Pack {
        code: "frm",
        name: "French (Middle, 1400-1600)",
        bytes: 2024948,
        sha256: "3fe6eed7108136a85dbbf53baf23d9fb435afe1262323f5fba7711b6c6048728",
    },
    Pack {
        code: "fry",
        name: "Western Frisian",
        bytes: 1906018,
        sha256: "7c9dbc47ef3848e33b623d1b927dd4d998ed39ff750fbc0f14aeb0f627b5da15",
    },
    Pack {
        code: "gla",
        name: "Scottish Gaelic",
        bytes: 3068307,
        sha256: "cb7b206d9f56b601d1666683ea8a516c8bb9c04362dbb874fc9665b339b72adf",
    },
    Pack {
        code: "gle",
        name: "Irish",
        bytes: 1181824,
        sha256: "2fe9ba6119aac7e2a20d6cfb69ed91afe9520f41bc7e3a903f84280f2663858d",
    },
    Pack {
        code: "glg",
        name: "Galician",
        bytes: 2554555,
        sha256: "7947619c5544d86849f563bd737ad90dbea9f5319fbd838c4747a7a1cd40b260",
    },
    Pack {
        code: "grc",
        name: "Greek (Ancient)",
        bytes: 2246328,
        sha256: "57b735557eeb67503803cf9f99786dd0b62417931352601952a0b1a23b6c4759",
    },
    Pack {
        code: "guj",
        name: "Gujarati",
        bytes: 1418394,
        sha256: "fa69658614b4946a9afae8853d67e0689838803dfa3d12c2e35ec53ee6f8df34",
    },
    Pack {
        code: "hat",
        name: "Haitian Creole",
        bytes: 1976902,
        sha256: "842e489a0e0eb12c72b5cb12e9e6624aae5928de1d097d76445474c263ce8b5a",
    },
    Pack {
        code: "heb",
        name: "Hebrew",
        bytes: 961404,
        sha256: "11f9e43ab227f786352a50f75c94c2e9906f1baba86d93276da19da7ce0904db",
    },
    Pack {
        code: "hin",
        name: "Hindi",
        bytes: 1122751,
        sha256: "4c73ffc59d497c186b19d1e90f5d721d678ea6b2e277b719bee4e2af12271825",
    },
    Pack {
        code: "hrv",
        name: "Croatian",
        bytes: 4103348,
        sha256: "9e515d9832ce259dbab550b1cc6b998f8b929faf2edacaaca981b05adb130571",
    },
    Pack {
        code: "hun",
        name: "Hungarian",
        bytes: 5296273,
        sha256: "35067e7cfe102dcdc953f9a758fdfaa6296b17a1ee6d874ee780fa306430b9fb",
    },
    Pack {
        code: "hye",
        name: "Armenian",
        bytes: 3463717,
        sha256: "b701d0d95799a716143dedb0197504e56f27a1bb133d6607ff5778c6988cb67c",
    },
    Pack {
        code: "iku",
        name: "Inuktitut",
        bytes: 2802266,
        sha256: "81ef16ee0fa0a3548c8a2d1ff10786e3278b6720e51cefd9a81a506f52660223",
    },
    Pack {
        code: "ind",
        name: "Indonesian",
        bytes: 1122661,
        sha256: "69786901da87ab8766c1ea7fbb10b28f2110c14da3f6c8f2735df131fba95d88",
    },
    Pack {
        code: "isl",
        name: "Icelandic",
        bytes: 2278973,
        sha256: "5ec828c363f3f0062c3caf08216677cf5f3c0c981b05fab445123a16b98f10a9",
    },
    Pack {
        code: "ita",
        name: "Italian",
        bytes: 2701314,
        sha256: "b8f89e1e785118dac4d51ae042c029a64edb5c3ee42ef73027a6d412748d8827",
    },
    Pack {
        code: "ita_old",
        name: "Italian (Old)",
        bytes: 3286123,
        sha256: "ee85de25acf70d5eeefdb3012424814a18ab368183099cc2c3f2fe2d3dce3b02",
    },
    Pack {
        code: "jav",
        name: "Javanese",
        bytes: 2982741,
        sha256: "46107b6bed3bf2d9617e5e192e52524e368242543fead1a7175c363dddd400f2",
    },
    Pack {
        code: "jpn",
        name: "Japanese",
        bytes: 2471260,
        sha256: "1f5de9236d2e85f5fdf4b3c500f2d4926f8d9449f28f5394472d9e8d83b91b4d",
    },
    Pack {
        code: "jpn_vert",
        name: "Japanese (vertical)",
        bytes: 3037480,
        sha256: "bf1e2640954691797e2dc14f38533e601b59ee37958698ae0f0b81dc6f09c71b",
    },
    Pack {
        code: "kan",
        name: "Kannada",
        bytes: 3608331,
        sha256: "bd31e6b6ae93271e3bcf5383d306d8eefbb91542937cd6d735a5930c970e61d8",
    },
    Pack {
        code: "kat",
        name: "Georgian",
        bytes: 2524769,
        sha256: "557abb6f1c68bc1b286f1bdd00bb6b82f85a427a91899807dab6c2f6c7986731",
    },
    Pack {
        code: "kat_old",
        name: "Georgian (Old)",
        bytes: 423167,
        sha256: "7b77290303d5d88db6adc5b679d1eb876e3c62f64c8999e919bcb4608af379bc",
    },
    Pack {
        code: "kaz",
        name: "Kazakh",
        bytes: 4734644,
        sha256: "fcc01eed3815a42b9c6321c4c9d3606f39b166cbf95ade98b7d8d12063eae53d",
    },
    Pack {
        code: "khm",
        name: "Khmer",
        bytes: 1446926,
        sha256: "47f110575341b322052f3becbefee61a3ecf1ef549352b5d4d33d28afe30d099",
    },
    Pack {
        code: "kir",
        name: "Kyrgyz",
        bytes: 9928497,
        sha256: "9777956300900b528d26932cf80693f95e75143433fb851d567194bcc38a31ae",
    },
    Pack {
        code: "kmr",
        name: "Kurdish (Kurmanji)",
        bytes: 3568615,
        sha256: "a3ef2daa828399ca8d6ae69edfa21ed21aa5abb3f868e2024d3235406312e7b6",
    },
    Pack {
        code: "kor",
        name: "Korean",
        bytes: 1677415,
        sha256: "6b85e11d9bbf07863b97b3523b1b112844c43e713df8b66418a081fd1060b3b2",
    },
    Pack {
        code: "kor_vert",
        name: "Korean (vertical)",
        bytes: 1113715,
        sha256: "c28f19dee36927baba5215fb793a3c00fff3ef2cfbcaa100122401d8f4374869",
    },
    Pack {
        code: "lao",
        name: "Lao",
        bytes: 6386744,
        sha256: "20124962e93e68121e02c49a949d1f9df5db87dd62e5e4aa578362ca532444f8",
    },
    Pack {
        code: "lat",
        name: "Latin",
        bytes: 3187463,
        sha256: "3859d8ba60404f4b79830622625bbc76fb4ee2808eac1ad360ffa77f0a533328",
    },
    Pack {
        code: "lav",
        name: "Latvian",
        bytes: 2717247,
        sha256: "7d9eaf22254b381f18b806cd6cb647862a397a7221adbce3e22cc421793a8847",
    },
    Pack {
        code: "lit",
        name: "Lithuanian",
        bytes: 3154896,
        sha256: "1e383df5b055583bc01cb5764ecdf74c540753f2cb3f8205e7105361da4bc989",
    },
    Pack {
        code: "ltz",
        name: "Luxembourgish",
        bytes: 2606426,
        sha256: "5516a78efa30c050da7eb0727fec3de59670c19735c00678a67fdad98e4b3ec1",
    },
    Pack {
        code: "mal",
        name: "Malayalam",
        bytes: 5275996,
        sha256: "bd05cbf1b197e7810d2903419aedb06f9ef77bfedf50b358673c1d18d707cdb4",
    },
    Pack {
        code: "mar",
        name: "Marathi",
        bytes: 2118233,
        sha256: "0ba3f2d116972e72fe9e176bc84c38e81dfb6670f4ed1f7f6c8e16a27da7cb61",
    },
    Pack {
        code: "mkd",
        name: "Macedonian",
        bytes: 1600188,
        sha256: "58622bf154830fa62103359938564aeb8112b929759e22a48224f3ecfaac34c6",
    },
    Pack {
        code: "mlt",
        name: "Maltese",
        bytes: 2308796,
        sha256: "3040b443d5d49e4183da0eaaf58d1033c3a616153d35d2342a56777f9bbb31c7",
    },
    Pack {
        code: "mon",
        name: "Mongolian",
        bytes: 2137042,
        sha256: "a151a3806d61ac43619cd383896d551ba5c3b07388ffec6fc83c8c604d677570",
    },
    Pack {
        code: "mri",
        name: "Maori",
        bytes: 862973,
        sha256: "8552be985e8fa0210d6d815d736151d645089b590a56f2d92391fb864e8bf9c9",
    },
    Pack {
        code: "msa",
        name: "Malay",
        bytes: 1747801,
        sha256: "e41a3e5febfec50c90371eb1cbb17a48b10cad387900e3420b1f134c1b766cba",
    },
    Pack {
        code: "mya",
        name: "Burmese",
        bytes: 4640561,
        sha256: "02aa6c25cfe9e583fa7b5d4131eac948f962308983f0f397df077dea58212b03",
    },
    Pack {
        code: "nep",
        name: "Nepali",
        bytes: 1002911,
        sha256: "280ba9450b4f21afbf5985e0de87857b75a972577c50ef0603c141ddde4f1cb8",
    },
    Pack {
        code: "nld",
        name: "Dutch",
        bytes: 6050296,
        sha256: "ced0e5e046a84c908a6aa7accbef9a232c4a5d9a8276691b81c6ee64d02963f6",
    },
    Pack {
        code: "nor",
        name: "Norwegian",
        bytes: 3610079,
        sha256: "0451eb4f8049ae78196806bf878a389a2f40f1386fe038568cf4441226ba6ef2",
    },
    Pack {
        code: "oci",
        name: "Occitan",
        bytes: 6322087,
        sha256: "747b9be4c5b80b4c730ce4f426a3b85060a1bb0de54cd566fcbdcb7f407940c9",
    },
    Pack {
        code: "ori",
        name: "Odia",
        bytes: 1480066,
        sha256: "36f3135e61d501a3acfad41f5fe60b8e791274fff4c5375c969fdcca980cdbac",
    },
    Pack {
        code: "pan",
        name: "Punjabi",
        bytes: 497721,
        sha256: "1ec0907fc3534065ea9ae190c6bb7ec9e5c74fd9d2fa996aaec7407f11ad8131",
    },
    Pack {
        code: "pol",
        name: "Polish",
        bytes: 4765518,
        sha256: "c4476cdbc0e33d898d32345122b7be1cbf85ace15f920f06c7714756e1ef79b2",
    },
    Pack {
        code: "por",
        name: "Portuguese",
        bytes: 1982756,
        sha256: "c4932b937207a9514b7514d518b931a99938c02a28a5a5a553f8599ed58b7deb",
    },
    Pack {
        code: "pus",
        name: "Pashto",
        bytes: 1772087,
        sha256: "f15550bfe3a20ca781cf5afb7b2af6ee769c6521cf4bb990848e72856b484600",
    },
    Pack {
        code: "que",
        name: "Quechua",
        bytes: 5026336,
        sha256: "9a3f78b010254ae237a7ca89e65e6d0c349221e437ac1575df1877e63a76bf57",
    },
    Pack {
        code: "ron",
        name: "Romanian",
        bytes: 2376323,
        sha256: "9adfde6b51ba4b97efd10ea37c3070fd3fc2bad7815e81f5c3c198cd96216cc9",
    },
    Pack {
        code: "rus",
        name: "Russian",
        bytes: 3861738,
        sha256: "e16e5e036cce1d9ec2b00063cf8b54472625b9e14d893a169e2b0dedeb4df225",
    },
    Pack {
        code: "san",
        name: "Sanskrit",
        bytes: 12404677,
        sha256: "a399958921c64e7d2319369ade1e74b4db2577744ac5bfeabd16663cf743b91c",
    },
    Pack {
        code: "sin",
        name: "Sinhala",
        bytes: 1727413,
        sha256: "bc75c6df2375a30c9f7e759fa7d4b58ae3ecf9ce72668a702d01acb13e551422",
    },
    Pack {
        code: "slk",
        name: "Slovak",
        bytes: 4427661,
        sha256: "fbcc400a9c74c6a13d922fcb1211b655d1b165387b675ed75cd2dbd756b974a5",
    },
    Pack {
        code: "slv",
        name: "Slovenian",
        bytes: 3003829,
        sha256: "b937632c17ce5fdf20535d25feec044da10a0c4c8234f4302d5606fd671cf60d",
    },
    Pack {
        code: "snd",
        name: "Sindhi",
        bytes: 1694035,
        sha256: "f25cae3b325b0a6dc4dd5d8aeb3ac40b129a18bdd92666317022ee198f7878a7",
    },
    Pack {
        code: "spa",
        name: "Spanish",
        bytes: 2294433,
        sha256: "6f2e04d02774a18f01bed44b1111f2cd7f3ba7ac9dc4373cd3f898a40ea6b464",
    },
    Pack {
        code: "spa_old",
        name: "Spanish (Old)",
        bytes: 2894755,
        sha256: "11d89a6763ff134ef8f7e41b995974d36fba5d1a807156d3202d7bf991bb1823",
    },
    Pack {
        code: "sqi",
        name: "Albanian",
        bytes: 1874705,
        sha256: "1d89621d9afe8ddf35b403ecf8951edea52c3a960d241f287b1a7b2c5ba8daac",
    },
    Pack {
        code: "srp",
        name: "Serbian",
        bytes: 2149931,
        sha256: "aa41ae3d9cc705e60d398ab38a5c3cc8b772c0d420c7d4f0859beb13d0e321b6",
    },
    Pack {
        code: "srp_latn",
        name: "Serbian (Latin)",
        bytes: 3281787,
        sha256: "54b2e001bd8a336bdffc249c1487b8b7cb712f0a031f2551930ba050ebefa2a0",
    },
    Pack {
        code: "sun",
        name: "Sundanese",
        bytes: 1369500,
        sha256: "fa939213b005b3477a1032b7222f63cd0b0ac486f83fd9554caf653d6e6c6ffe",
    },
    Pack {
        code: "swa",
        name: "Swahili",
        bytes: 2167651,
        sha256: "395439d1ec308535066cbaea9b15e0e4cc81f8609170af76ab7e7e8d3ec42f3e",
    },
    Pack {
        code: "swe",
        name: "Swedish",
        bytes: 4167034,
        sha256: "f7304988d41f833efebcc2d529df54b1903ecebbc3da1faabd19a0fddd4fe586",
    },
    Pack {
        code: "syr",
        name: "Syriac",
        bytes: 2207208,
        sha256: "7435b43466da8343619bdc43c4da8dc8581f21aab3337fa1bd22ed8a8c10af04",
    },
    Pack {
        code: "tam",
        name: "Tamil",
        bytes: 3237963,
        sha256: "d02fbec24be4b07e32e80d0ccfc3b6b67a3c5d61c9d0a7c8532677990912c6ec",
    },
    Pack {
        code: "tat",
        name: "Tatar",
        bytes: 1072896,
        sha256: "7ad946d753f4b9688e4e4bcf525a931f486da4aaa9020e32b6a7262718bdf093",
    },
    Pack {
        code: "tel",
        name: "Telugu",
        bytes: 2769654,
        sha256: "d10691fddd5b67802e1c12800ebb321d3b8bcd8d24a2ac3ff206f93188c04ab5",
    },
    Pack {
        code: "tgk",
        name: "Tajik",
        bytes: 2602685,
        sha256: "7b32ed1374649b9b44b9a20ed07f1e86c1e1004c3bd11ce182496a47a18cb321",
    },
    Pack {
        code: "tha",
        name: "Thai",
        bytes: 1072600,
        sha256: "294227cc2d1292b0acb28d61d4115c88252b96d466ca90b417cf4cf0c67bf07c",
    },
    Pack {
        code: "tir",
        name: "Tigrinya",
        bytes: 378822,
        sha256: "73d7430c22a062b061f603a2c1d70c4ea4aa092f7a932a97921926e15b4fdb3d",
    },
    Pack {
        code: "ton",
        name: "Tongan",
        bytes: 947249,
        sha256: "23d36448a3c59499da59502419ffcaa279056fa369a0fa2c798e066321d01bd1",
    },
    Pack {
        code: "tur",
        name: "Turkish",
        bytes: 4550554,
        sha256: "7393381111e1152420fc4092cb44eef4237580d21b92bf30d7d221aad192c6b7",
    },
    Pack {
        code: "uig",
        name: "Uyghur",
        bytes: 2794272,
        sha256: "163b360268c39fc69e0c0dc9299c06dfe7a6c51ef8f33766da75022217b69b22",
    },
    Pack {
        code: "ukr",
        name: "Ukrainian",
        bytes: 3825102,
        sha256: "d59e53e2bded32f4445f124b4b00240fcac7e8044c003ab822ccb94f0b3db59b",
    },
    Pack {
        code: "urd",
        name: "Urdu",
        bytes: 1398718,
        sha256: "62e8250ce2a994106e313a82e26a516a39e2cf159d0ce3c5b5008387fd0d555f",
    },
    Pack {
        code: "uzb",
        name: "Uzbek",
        bytes: 6470703,
        sha256: "0e39021eefe692906b50d9bd22e580fa57fe43fa35db6846ad8b6369ddc21ea8",
    },
    Pack {
        code: "uzb_cyrl",
        name: "Uzbek (Cyrillic)",
        bytes: 1563501,
        sha256: "a2fe0b3f0eb4a5e46252470160ad51cc5f3c4c0f390ba67d1911beb0dbe80cc8",
    },
    Pack {
        code: "vie",
        name: "Vietnamese",
        bytes: 531275,
        sha256: "79df64caf7bcfb2a27df5042ecb6121e196eada34da774956995747636d5bfa1",
    },
    Pack {
        code: "yid",
        name: "Yiddish",
        bytes: 545606,
        sha256: "f9a3d74076460a505dc0c6b59a0f5bd1108c6436d570954223b3695988d1666d",
    },
    Pack {
        code: "yor",
        name: "Yoruba",
        bytes: 963400,
        sha256: "17ab3855f1ba9056183759a84e4c11cebf417a7d3c8c2cdcc37537d7fbffba3d",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pack_is_a_plain_code_pinned_by_its_hash() {
        let mut codes: Vec<&str> = PACKS.iter().map(|p| p.code).collect();
        codes.dedup();
        assert_eq!(codes.len(), PACKS.len());
        assert!(codes.windows(2).all(|w| w[0] < w[1]));
        for pack in PACKS {
            assert!(crate::ocr::plain_code(pack.code), "{}", pack.code);
            assert!(
                pack.bytes > 100_000 && pack.bytes < 50_000_000,
                "{}",
                pack.code
            );
            assert!(
                pack.sha256.len() == 64 && pack.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "{}",
                pack.code
            );
        }
        assert_eq!(pack("deu").unwrap().name, "German");
        assert!(pack("osd").is_none());
        assert!(
            pack("ita")
                .unwrap()
                .url()
                .ends_with(&format!("{COMMIT}/ita.traineddata"))
        );
    }
}
