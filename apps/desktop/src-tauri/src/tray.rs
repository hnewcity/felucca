//! Tray menu text. The menu exists before the window loads its catalogs, so
//! these few labels are kept here in every language the product ships.

use felucca_i18n::Language;

pub struct Labels {
    pub open: &'static str,
    pub quit: &'static str,
    /// `{online}` of `{total}` connections online.
    pub summary: &'static str,
    pub empty: &'static str,
}

pub fn labels(language: Language) -> Labels {
    match language {
        Language::Chinese => Labels {
            open: "打开 Felucca",
            quit: "退出",
            summary: "{online}/{total} 个连接在线",
            empty: "尚未添加连接",
        },
        Language::Japanese => Labels {
            open: "Felucca を開く",
            quit: "終了",
            summary: "{total} 件中 {online} 件の接続がオンライン",
            empty: "接続はまだありません",
        },
        Language::Korean => Labels {
            open: "Felucca 열기",
            quit: "종료",
            summary: "연결 {total}개 중 {online}개 온라인",
            empty: "아직 연결이 없습니다",
        },
        Language::French => Labels {
            open: "Ouvrir Felucca",
            quit: "Quitter",
            summary: "{online} connexion(s) sur {total} en ligne",
            empty: "Aucune connexion",
        },
        Language::Spanish => Labels {
            open: "Abrir Felucca",
            quit: "Salir",
            summary: "{online} de {total} conexiones en línea",
            empty: "Todavía no hay conexiones",
        },
        Language::Russian => Labels {
            open: "Открыть Felucca",
            quit: "Выйти",
            summary: "В сети: {online} из {total}",
            empty: "Подключений пока нет",
        },
        _ => Labels {
            open: "Open Felucca",
            quit: "Quit",
            summary: "{online} of {total} connections online",
            empty: "No connections yet",
        },
    }
}

pub fn summary(language: Language, online: usize, total: usize) -> String {
    let labels = labels(language);
    if total == 0 {
        return labels.empty.to_owned();
    }
    labels
        .summary
        .replace("{online}", &online.to_string())
        .replace("{total}", &total.to_string())
}
