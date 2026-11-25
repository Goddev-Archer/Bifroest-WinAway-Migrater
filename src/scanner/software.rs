use std::{
    collections::HashSet,
    path::Path,
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::process::Command;

use crate::model::DiscoveredSoftware;

#[allow(dead_code)]
#[derive(Clone)]
struct CatalogEntry {
    name: &'static str,
    id: &'static str,
    winget_keywords: &'static [&'static str],
    flatpak_id: Option<&'static str>,
    native_pkg: Option<&'static str>,
}

// Umfangreicher Software-Katalog: Windows -> Flathub / Arch Pacman Mappings
#[allow(dead_code)]
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

/// Parses native Linux explicitly installed package lists (`pacman -Qqe`, `apt-mark showmanual`, `dnf repoquery`).
pub fn parse_linux_native_packages(raw_list: &str, source_label: &str) -> Vec<DiscoveredSoftware> {
    let mut found = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut matched_tokens = HashSet::new();

    let tokens: Vec<String> = raw_list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
        .collect();
    let lc_tokens: HashSet<String> = tokens.iter().map(|t| t.to_ascii_lowercase()).collect();

    for entry in EXTENDED_CATALOG {
        let mut matched_tok = None;
        if let Some(pkg) = entry.native_pkg {
            let base = pkg.trim_end_matches("-bin").trim_end_matches("-launcher");
            for tok in &lc_tokens {
                if tok == pkg || tok == base || tok == &format!("{base}-browser") {
                    matched_tok = Some(tok.clone());
                    break;
                }
            }
        }
        if matched_tok.is_none() {
            for kw in entry.winget_keywords {
                if lc_tokens.contains(*kw) {
                    matched_tok = Some((*kw).to_string());
                    break;
                }
            }
        }

        if let Some(tok) = matched_tok {
            matched_tokens.insert(tok);
            if seen_ids.insert(entry.id.to_string()) {
                found.push(DiscoveredSoftware {
                    name: entry.name.to_string(),
                    id: entry.id.to_string(),
                    source: source_label.to_string(),
                    flatpak_id: entry.flatpak_id.map(|s| s.to_string()),
                    native_pkg: entry.native_pkg.map(|s| s.to_string()),
                    will_reinstall: true,
                });
            }
        }
    }

    // Preserve explicit non-library user packages (exact-name cross-distro fallback, bounded)
    let mut extra_count = 0usize;
    for raw_tok in tokens {
        let lc = raw_tok.to_ascii_lowercase();
        if matched_tokens.contains(&lc) {
            continue;
        }
        if lc.starts_with("lib")
            || lc.ends_with("-dev")
            || lc.ends_with("-devel")
            || lc.ends_with("-common")
            || lc.starts_with("linux-")
            || lc.starts_with("fonts-")
            || lc.starts_with("ttf-")
            || matches!(
                lc.as_str(),
                "base" | "base-devel" | "filesystem" | "systemd" | "systemd-sysv" | "sudo" | "btrfs-progs"
            )
        {
            continue;
        }
        let id = format!("linux.native.{lc}");
        if seen_ids.insert(id.clone()) {
            found.push(DiscoveredSoftware {
                name: raw_tok.clone(),
                id,
                source: source_label.to_string(),
                flatpak_id: None,
                native_pkg: Some(raw_tok),
                will_reinstall: true,
            });
            extra_count += 1;
            if extra_count >= 40 {
                break;
            }
        }
    }

    found
}

/// Parses `flatpak list --app --columns=application,name` output into `DiscoveredSoftware`.
pub fn parse_flatpak_app_list(raw_tsv: &str) -> Vec<DiscoveredSoftware> {
    let mut apps = Vec::new();
    let mut seen = HashSet::new();

    for line in raw_tsv.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, '\t');
        let app_id = parts.next().unwrap_or("").trim();
        if app_id.is_empty() || !app_id.contains('.') {
            continue;
        }
        let name = parts
            .next()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(app_id);

        if seen.insert(app_id.to_string()) {
            apps.push(DiscoveredSoftware {
                name: name.to_string(),
                id: app_id.to_string(),
                source: "Flatpak".to_string(),
                flatpak_id: Some(app_id.to_string()),
                native_pkg: None,
                will_reinstall: true,
            });
        }
    }

    apps
}

pub fn scan_installed_software(user_profile: &Path) -> Vec<DiscoveredSoftware> {
    let mut found = Vec::new();
    let mut seen_ids = HashSet::new();

    // 1. Scan via winget list (Windows 11)
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

    // 1b. Scan via Flatpak & native Linux package managers (pacman / apt / dnf)
    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = Command::new("flatpak")
            .args(["list", "--app", "--columns=application,name"])
            .output()
        {
            if out.status.success() {
                for app in parse_flatpak_app_list(&String::from_utf8_lossy(&out.stdout)) {
                    if let Some(fp) = &app.flatpak_id {
                        seen_ids.insert(fp.clone());
                    }
                    if seen_ids.insert(app.id.clone()) {
                        found.push(app);
                    }
                }
            }
        }

        let pm_probes: &[(&str, &[&str], &str)] = &[
            ("pacman", &["-Qqe"], "Pacman Explicit DB"),
            ("apt-mark", &["showmanual"], "APT Manual DB"),
            ("dnf", &["repoquery", "--userinstalled", "--qf", "%{name}"], "DNF Userinstalled DB"),
        ];
        for &(bin, args, label) in pm_probes {
            if let Ok(out) = Command::new(bin).args(args).output() {
                if out.status.success() {
                    for pkg in parse_linux_native_packages(&String::from_utf8_lossy(&out.stdout), label) {
                        let skip_fp = pkg
                            .flatpak_id
                            .as_ref()
                            .map(|fp| seen_ids.contains(fp))
                            .unwrap_or(false);
                        if !skip_fp && seen_ids.insert(pkg.id.clone()) {
                            found.push(pkg);
                        }
                    }
                    break;
                }
            }
        }
    }

    // 2. Fallback: Suche in bekannten AppData- (Windows) bzw. XDG/.var-Ordnern (Linux)
    let appdata_roaming = user_profile.join("AppData").join("Roaming");
    let appdata_local = user_profile.join("AppData").join("Local");
    let xdg_config = user_profile.join(".config");
    let var_app = user_profile.join(".var").join("app");

    let fallback_checks: &[(&str, &str, &str, Option<&str>, Option<&str>)] = &[
        ("Visual Studio Code", "Microsoft.VisualStudioCode", "Code", Some("com.visualstudio.code"), Some("code")),
        ("Brave Browser", "Brave.Brave", "BraveSoftware\\Brave-Browser", Some("com.brave.Browser"), Some("brave-bin")),
        ("Discord", "Discord.Discord", "Discord", Some("com.discordapp.Discord"), Some("discord")),
        ("Obsidian", "Obsidian.Obsidian", "Obsidian", Some("md.obsidian.Obsidian"), Some("obsidian")),
        ("GIMP", "GIMP.GIMP", "GIMP", Some("org.gimp.GIMP"), Some("gimp")),
        ("VLC", "VideoLAN.VLC", "vlc", Some("org.videolan.VLC"), Some("vlc")),
    ];

    for &(name, id, subpath, flatpak, native) in fallback_checks {
        let already_fp = flatpak.map(|fp| seen_ids.contains(fp)).unwrap_or(false);
        if !already_fp && !seen_ids.contains(id) {
            let win_hit = appdata_roaming.join(subpath).exists() || appdata_local.join(subpath).exists();
            let linux_cfg_name = subpath.split('\\').next().unwrap_or(subpath);
            let linux_hit = xdg_config.join(linux_cfg_name).exists()
                || xdg_config.join(linux_cfg_name.to_ascii_lowercase()).exists()
                || flatpak.map(|fp| var_app.join(fp).exists()).unwrap_or(false);
            if win_hit || linux_hit {
                seen_ids.insert(id.to_string());
                found.push(DiscoveredSoftware {
                    name: name.to_string(),
                    id: id.to_string(),
                    source: if cfg!(target_os = "linux") {
                        "Heuristik (~/.config / ~/.var/app)".to_string()
                    } else {
                        "Heuristik (AppData)".to_string()
                    },
                    flatpak_id: flatpak.map(|s| s.to_string()),
                    native_pkg: native.map(|s| s.to_string()),
                    will_reinstall: true,
                });
            }
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_software_and_flatpak_parsers() {
        let native_out = "git\nneovim\nfirefox\nlibssl3\nbrave-browser\ntmux\n";
        let pkgs = parse_linux_native_packages(native_out, "APT Manual DB");
        assert!(pkgs.iter().any(|p| p.id == "Git.Git"));
        assert!(pkgs.iter().any(|p| p.id == "Mozilla.Firefox"));
        assert!(pkgs.iter().any(|p| p.id == "Brave.Brave"));
        assert!(pkgs.iter().any(|p| p.name == "neovim" && p.native_pkg.as_deref() == Some("neovim")));
        assert!(pkgs.iter().any(|p| p.name == "tmux"));
        assert!(!pkgs.iter().any(|p| p.name == "libssl3"));

        let flatpak_tsv = "com.visualstudio.code\tVisual Studio Code\nmd.obsidian.Obsidian\tObsidian\n";
        let fps = parse_flatpak_app_list(flatpak_tsv);
        assert_eq!(fps.len(), 2);
        assert_eq!(fps[0].flatpak_id.as_deref(), Some("com.visualstudio.code"));
        assert_eq!(fps[1].name, "Obsidian");
    }
}
