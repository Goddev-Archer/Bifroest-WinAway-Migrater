use std::{
    collections::HashSet,
    path::Path,
};

#[cfg(target_os = "windows")]
use std::process::Command;

use crate::model::DiscoveredSoftware;

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone)]
struct CatalogEntry {
    name: &'static str,
    id: &'static str,
    winget_keywords: &'static [&'static str],
    flatpak_id: Option<&'static str>,
    native_pkg: Option<&'static str>,
}

// Umfangreicher Software-Katalog: Windows -> Flathub / Arch Pacman Mappings
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const EXTENDED_CATALOG: &[CatalogEntry] = &[
    // IDEs & Entwickler-Tools
    CatalogEntry {
        name: "Visual Studio Code",
        id: "Microsoft.VisualStudioCode",
        winget_keywords: &["visualstudiocode", "microsoft.visualstudiocode", "code"],
        flatpak_id: Some("com.visualstudio.code"),
        native_pkg: Some("code"),
    },
    CatalogEntry {
        name: "CLion",
        id: "JetBrains.CLion",
        winget_keywords: &["jetbrains.clion", "clion"],
        flatpak_id: Some("com.jetbrains.CLion"),
        native_pkg: Some("clion"),
    },
    CatalogEntry {
        name: "DataGrip",
        id: "JetBrains.DataGrip",
        winget_keywords: &["jetbrains.datagrip", "datagrip"],
        flatpak_id: Some("com.jetbrains.DataGrip"),
        native_pkg: Some("datagrip"),
    },
    CatalogEntry {
        name: "IntelliJ IDEA",
        id: "JetBrains.IntelliJIDEA",
        winget_keywords: &["jetbrains.intellij", "idea-u", "idea-c"],
        flatpak_id: Some("com.jetbrains.IntelliJ-IDEA-Community"),
        native_pkg: Some("intellij-idea-community-edition"),
    },
    CatalogEntry {
        name: "JetBrains Toolbox",
        id: "JetBrains.Toolbox",
        winget_keywords: &["jetbrains.toolbox", "toolbox"],
        flatpak_id: Some("com.jetbrains.Toolbox"),
        native_pkg: Some("jetbrains-toolbox"),
    },
    CatalogEntry {
        name: "Arduino IDE",
        id: "ArduinoSA.IDE",
        winget_keywords: &["arduiposa.ide", "arduino.ide", "arduino ide"],
        flatpak_id: Some("cc.arduino.IDE2"),
        native_pkg: Some("arduino-ide"),
    },
    CatalogEntry {
        name: "Blockbench",
        id: "JannisX11.Blockbench",
        winget_keywords: &["jannisx11.blockbench", "blockbench"],
        flatpak_id: Some("net.blockbench.Blockbench"),
        native_pkg: Some("blockbench"),
    },
    CatalogEntry {
        name: "GitHub Desktop",
        id: "GitHub.GitHubDesktop",
        winget_keywords: &["github.githubdesktop", "github desktop"],
        flatpak_id: Some("io.github.shifteight.desktop"),
        native_pkg: Some("github-desktop-bin"),
    },
    CatalogEntry {
        name: "Git",
        id: "Git.Git",
        winget_keywords: &["git.git"],
        flatpak_id: None,
        native_pkg: Some("git"),
    },

    // Web-Browser
    CatalogEntry {
        name: "Brave Browser",
        id: "Brave.Brave",
        winget_keywords: &["brave.brave", "brave"],
        flatpak_id: Some("com.brave.Browser"),
        native_pkg: Some("brave-bin"),
    },
    CatalogEntry {
        name: "Google Chrome",
        id: "Google.Chrome",
        winget_keywords: &["google.chrome", "google chrome"],
        flatpak_id: Some("com.google.Chrome"),
        native_pkg: Some("google-chrome"),
    },
    CatalogEntry {
        name: "Mozilla Firefox",
        id: "Mozilla.Firefox",
        winget_keywords: &["mozilla.firefox", "firefox"],
        flatpak_id: Some("org.mozilla.firefox"),
        native_pkg: Some("firefox"),
    },

    // Medien & Kreativ-Software
    CatalogEntry {
        name: "GIMP",
        id: "GIMP.GIMP",
        winget_keywords: &["gimp", "gimp-3", "gimp.gimp"],
        flatpak_id: Some("org.gimp.GIMP"),
        native_pkg: Some("gimp"),
    },
    CatalogEntry {
        name: "Kdenlive",
        id: "KDE.Kdenlive",
        winget_keywords: &["kde.kdenlive", "kdenlive"],
        flatpak_id: Some("org.kde.kdenlive"),
        native_pkg: Some("kdenlive"),
    },
    CatalogEntry {
        name: "VLC media player",
        id: "VideoLAN.VLC",
        winget_keywords: &["videolan.vlc", "vlc"],
        flatpak_id: Some("org.videolan.VLC"),
        native_pkg: Some("vlc"),
    },
    CatalogEntry {
        name: "Obsidian",
        id: "Obsidian.Obsidian",
        winget_keywords: &["obsidian.obsidian", "obsidian"],
        flatpak_id: Some("md.obsidian.Obsidian"),
        native_pkg: Some("obsidian"),
    },
    CatalogEntry {
        name: "Figma",
        id: "Figma.Figma",
        winget_keywords: &["figma.figma", "figma"],
        flatpak_id: Some("io.github.Figma_Linux.figma_linux"),
        native_pkg: Some("figma-linux"),
    },
    CatalogEntry {
        name: "Spotify",
        id: "Spotify.Spotify",
        winget_keywords: &["spotify.spotify", "spotify"],
        flatpak_id: Some("com.spotify.Client"),
        native_pkg: Some("spotify-launcher"),
    },

    // Kommunikation & Gaming
    CatalogEntry {
        name: "Discord",
        id: "Discord.Discord",
        winget_keywords: &["discord.discord", "discord"],
        flatpak_id: Some("com.discordapp.Discord"),
        native_pkg: Some("discord"),
    },
    CatalogEntry {
        name: "Element (Matrix Client)",
        id: "Element.Element",
        winget_keywords: &["element.element", "element"],
        flatpak_id: Some("im.riot.Riot"),
        native_pkg: Some("element-desktop"),
    },
    CatalogEntry {
        name: "Steam",
        id: "Valve.Steam",
        winget_keywords: &["valve.steam", "steam"],
        flatpak_id: Some("com.valvesoftware.Steam"),
        native_pkg: Some("steam"),
    },

    // Utilities & Behörden
    CatalogEntry {
        name: "AusweisApp",
        id: "Governikus.AusweisApp",
        winget_keywords: &["governikus.ausweisapp", "ausweisapp"],
        flatpak_id: Some("de.bund.ausweisapp.ausweisapp2"),
        native_pkg: Some("ausweisapp2"),
    },
    CatalogEntry {
        name: "Cloudflare WARP",
        id: "Cloudflare.Warp",
        winget_keywords: &["cloudflare.warp", "cloudflare one"],
        flatpak_id: None,
        native_pkg: Some("cloudflare-warp-bin"),
    },
];

pub fn scan_installed_software(user_profile: &Path) -> Vec<DiscoveredSoftware> {
    let mut found = Vec::new();
    let mut seen_ids = HashSet::new();

    // 1. Scan via winget list (Die zuverlässigste Quelle auf Windows 11)
    #[cfg(target_os = "windows")]
    if let Ok(output) = Command::new("winget.exe")
        .args(["list", "--accept-source-agreements"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();

            for entry in EXTENDED_CATALOG {
                let mut matched = false;
                for kw in entry.winget_keywords {
                    if stdout.contains(kw) {
                        matched = true;
                        break;
                    }
                }

                if matched && seen_ids.insert(entry.id.to_string()) {
                    found.push(DiscoveredSoftware {
                        name: entry.name.to_string(),
                        id: entry.id.to_string(),
                        source: "Winget Package Database".to_string(),
                        flatpak_id: entry.flatpak_id.map(|s| s.to_string()),
                        native_pkg: entry.native_pkg.map(|s| s.to_string()),
                        will_reinstall: true,
                    });
                }
            }
        }
    }

    // 2. Fallback: Suche in bekannten AppData-Ordnern
    let appdata_roaming = user_profile.join("AppData").join("Roaming");
    let appdata_local = user_profile.join("AppData").join("Local");

    let fallback_checks: &[(&str, &str, &str, Option<&str>, Option<&str>)] = &[
        ("Visual Studio Code", "Microsoft.VisualStudioCode", "Code", Some("com.visualstudio.code"), Some("code")),
        ("Brave Browser", "Brave.Brave", "BraveSoftware\\Brave-Browser", Some("com.brave.Browser"), Some("brave-bin")),
        ("Discord", "Discord.Discord", "Discord", Some("com.discordapp.Discord"), Some("discord")),
        ("Obsidian", "Obsidian.Obsidian", "Obsidian", Some("md.obsidian.Obsidian"), Some("obsidian")),
        ("GIMP", "GIMP.GIMP", "GIMP", Some("org.gimp.GIMP"), Some("gimp")),
        ("VLC", "VideoLAN.VLC", "vlc", Some("org.videolan.VLC"), Some("vlc")),
    ];

    for &(name, id, subpath, flatpak, native) in fallback_checks {
        if !seen_ids.contains(id) {
            if appdata_roaming.join(subpath).exists() || appdata_local.join(subpath).exists() {
                seen_ids.insert(id.to_string());
                found.push(DiscoveredSoftware {
                    name: name.to_string(),
                    id: id.to_string(),
                    source: "Heuristik (AppData)".to_string(),
                    flatpak_id: flatpak.map(|s| s.to_string()),
                    native_pkg: native.map(|s| s.to_string()),
                    will_reinstall: true,
                });
            }
        }
    }

    found
}
