//! B20: synthetic Jellyfin data folders for the importer tests, in both database layouts.
//!
//! The DDL is hand-written: only the tables and columns the importer may read, with Jellyfin's
//! names and value formats (uppercase GUID text in `jellyfin.db`, 16-byte .NET-order GUID blobs
//! in the old `library.db`), plus the secret tables and columns. Those hold canary strings that
//! must never reach Rustyfin's database, its log or an API answer.

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::UNIX_EPOCH;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use uuid::Uuid;

/// Every canary starts with this.
pub const CANARY: &str = "CANARY-B20-";

/// Jellyfin's item id for detached user data (`BaseItemRepository.PlaceholderId`).
pub const DETACHED: Uuid = Uuid::from_u128(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Jellyfin 10.11 and later: users and library in `jellyfin.db`.
    New,
    /// Jellyfin 10.10 and earlier: users in `jellyfin.db`, the library in `library.db`.
    Old,
}

#[derive(Clone, Debug)]
pub struct JfUser {
    pub id: Uuid,
    pub internal_id: i64,
    pub name: String,
    pub admin: bool,
}

impl JfUser {
    pub fn new(internal_id: i64, name: &str, admin: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            internal_id,
            name: name.to_string(),
            admin,
        }
    }
}

#[derive(Clone, Debug)]
pub struct JfItem {
    pub id: Uuid,
    /// Last part of the .NET type name: "Movie", "Series", "Season", "Episode", "Audio", "Folder".
    pub kind: &'static str,
    pub name: String,
    pub path: Option<String>,
    pub parent: Option<Uuid>,
    pub series: Option<Uuid>,
    pub season: Option<Uuid>,
    pub index: Option<i32>,
    pub parent_index: Option<i32>,
    /// Provider ids as Jellyfin names them ("Tmdb", "Tvdb", "Imdb"), values as stored.
    pub providers: Vec<(&'static str, String)>,
    /// The old layout's `UserDataKey`.
    pub user_data_key: String,
    pub meta: JfMeta,
    /// Credits in the order written (`PeopleBaseItemMap`; old layout `People`).
    pub people: Vec<JfCredit>,
    /// Image rows in the order written (`BaseItemImageInfos`; old layout `TypedBaseItems.Images`).
    pub images: Vec<JfImage>,
}

/// Jellyfin's metadata columns of an item, as stored (dates as text, lists pipe-joined).
#[derive(Clone, Debug, Default)]
pub struct JfMeta {
    pub forced_sort_name: Option<&'static str>,
    pub original_title: Option<&'static str>,
    pub overview: Option<&'static str>,
    pub tagline: Option<&'static str>,
    pub official_rating: Option<&'static str>,
    pub community_rating: Option<f64>,
    pub critic_rating: Option<f64>,
    /// New layout text, e.g. "2001-05-04 00:00:00"; the old layout gets a trailing "Z".
    pub premiere_date: Option<&'static str>,
    pub end_date: Option<&'static str>,
    pub production_year: Option<i32>,
    pub genres: Option<&'static str>,
    pub studios: Option<&'static str>,
    pub tags: Option<&'static str>,
}

/// One credit of an item.
#[derive(Clone, Debug)]
pub struct JfCredit {
    pub name: &'static str,
    /// `PersonType`, e.g. "Actor", "Director"; None stores NULL.
    pub kind: Option<&'static str>,
    /// "" means none, as Jellyfin stores it.
    pub role: &'static str,
    pub list_order: Option<i32>,
    pub sort_order: Option<i32>,
}

/// One image row of an item.
#[derive(Clone, Debug)]
pub struct JfImage {
    /// As Jellyfin stores it: absolute, or starting with `%MetadataPath%`.
    pub path: String,
    /// Jellyfin's `ImageType` number: 0 Primary, 1 Art, 2 Backdrop, 3 Banner, 4 Logo, 5 Thumb,
    /// 6 Disc, 7 Box, 8 Screenshot, 9 Menu, 10 Chapter, 11 BoxRear, 12 Profile.
    pub kind: i32,
    pub width: i32,
    pub height: i32,
    pub blurhash: Option<&'static str>,
}

/// The start of a JPEG file; synthetic images are this plus a marker.
pub const JPEG_HEAD: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00,
];
/// The start of a PNG file.
pub const PNG_HEAD: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00,
];

/// Writes a synthetic image `head` + `marker` at `path`, with its parent folders; returns the bytes.
pub fn write_image(path: &Path, head: &[u8], marker: &str) -> Vec<u8> {
    let mut bytes = head.to_vec();
    bytes.extend_from_slice(marker.as_bytes());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}

fn image_type_name(kind: i32) -> &'static str {
    [
        "Primary",
        "Art",
        "Backdrop",
        "Banner",
        "Logo",
        "Thumb",
        "Disc",
        "Box",
        "Screenshot",
        "Menu",
        "Chapter",
        "BoxRear",
        "Profile",
    ][kind as usize]
}

/// The old layout's `Images` text: `Path*Ticks*Type*Width*Height*Blurhash` entries joined by `|`,
/// with `*` and `|` in a blurhash written as `/` and `\`.
fn old_images(images: &[JfImage]) -> Option<String> {
    let entries: Vec<String> = images
        .iter()
        .map(|i| {
            let hash = i
                .blurhash
                .unwrap_or("")
                .replace('*', "/")
                .replace('|', "\\");
            let kind = image_type_name(i.kind);
            format!(
                "{}*637000000000000000*{kind}*{}*{}*{hash}",
                i.path, i.width, i.height
            )
        })
        .collect();
    (!entries.is_empty()).then(|| entries.join("|"))
}

impl JfItem {
    pub fn new(kind: &'static str, name: &str, path: Option<&str>) -> Self {
        let id = Uuid::new_v4();
        Self {
            id,
            kind,
            name: name.to_string(),
            path: path.map(str::to_string),
            parent: None,
            series: None,
            season: None,
            index: None,
            parent_index: None,
            providers: Vec::new(),
            user_data_key: id.simple().to_string(),
            meta: JfMeta::default(),
            people: Vec::new(),
            images: Vec::new(),
        }
    }

    pub fn meta(mut self, meta: JfMeta) -> Self {
        self.meta = meta;
        self
    }

    /// Adds a credit with `ListOrder` `list_order` and no `SortOrder`.
    pub fn credit(
        mut self,
        name: &'static str,
        kind: Option<&'static str>,
        role: &'static str,
        list_order: Option<i32>,
    ) -> Self {
        self.people.push(JfCredit {
            name,
            kind,
            role,
            list_order,
            sort_order: None,
        });
        self
    }

    pub fn image(
        mut self,
        path: &str,
        kind: i32,
        size: (i32, i32),
        blurhash: Option<&'static str>,
    ) -> Self {
        self.images.push(JfImage {
            path: path.to_string(),
            kind,
            width: size.0,
            height: size.1,
            blurhash,
        });
        self
    }

    pub fn parent(mut self, parent: Uuid) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Sets the series a season belongs to (also its parent).
    pub fn in_series(mut self, series: Uuid) -> Self {
        self.series = Some(series);
        self.parent = Some(series);
        self
    }

    /// Sets the series and season an episode belongs to (its parent is the season).
    pub fn in_season(mut self, series: Uuid, season: Uuid) -> Self {
        self.series = Some(series);
        self.season = Some(season);
        self.parent = Some(season);
        self
    }

    pub fn numbers(mut self, parent_index: Option<i32>, index: Option<i32>) -> Self {
        self.parent_index = parent_index;
        self.index = index;
        self
    }

    pub fn provider(mut self, name: &'static str, value: &str) -> Self {
        self.providers.push((name, value.to_string()));
        self
    }
}

/// One `UserData` row (old layout: one `UserDatas` row, keyed by the item's `UserDataKey`).
#[derive(Clone, Debug, Default)]
pub struct JfUserData {
    pub user: Uuid,
    /// The item's id; `DETACHED`, or any id that is no item, gives a row without an item.
    pub item: Uuid,
    /// New layout `CustomDataKey`; `None` = the item's `user_data_key`.
    pub custom_key: Option<String>,
    pub played: bool,
    pub play_count: i32,
    pub position: i64,
    /// `YYYY-MM-DD HH:MM:SS[.fffffff]`; the old layout gets Jellyfin's trailing `Z`.
    pub last_played: Option<&'static str>,
    pub favorite: bool,
    pub rating: Option<f64>,
    /// New layout only; the old one has no such column.
    pub likes: Option<bool>,
    pub audio: Option<i32>,
    pub subtitle: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct JfStream {
    pub item: Uuid,
    pub index: i32,
    /// "Video", "Audio" or "Subtitle".
    pub kind: &'static str,
    pub codec: &'static str,
    pub language: Option<&'static str>,
    pub title: Option<&'static str>,
    pub external: bool,
    pub path: Option<String>,
    pub default: bool,
    pub forced: bool,
}

impl JfStream {
    pub fn new(item: Uuid, index: i32, kind: &'static str, codec: &'static str) -> Self {
        Self {
            item,
            index,
            kind,
            codec,
            language: None,
            title: None,
            external: false,
            path: None,
            default: false,
            forced: false,
        }
    }

    pub fn language(mut self, language: &'static str) -> Self {
        self.language = Some(language);
        self
    }

    pub fn title(mut self, title: &'static str) -> Self {
        self.title = Some(title);
        self
    }

    pub fn external(mut self, path: &str) -> Self {
        self.external = true;
        self.path = Some(path.to_string());
        self
    }

    pub fn flags(mut self, default: bool, forced: bool) -> Self {
        self.default = default;
        self.forced = forced;
        self
    }
}

/// A Jellyfin library folder `root/default/<name>/`.
#[derive(Clone, Debug)]
pub struct JfLibrary {
    pub name: &'static str,
    /// "movies", "tvshows" or "music".
    pub collection_type: &'static str,
    pub paths: Vec<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct JfSource {
    pub users: Vec<JfUser>,
    pub items: Vec<JfItem>,
    pub user_data: Vec<JfUserData>,
    pub streams: Vec<JfStream>,
    pub libraries: Vec<JfLibrary>,
}

impl JfSource {
    pub fn user(&self, name: &str) -> &JfUser {
        self.users
            .iter()
            .find(|u| u.name == name)
            .expect("no such user")
    }

    pub fn item(&self, name: &str) -> &JfItem {
        self.items
            .iter()
            .find(|i| i.name == name)
            .expect("no such item")
    }

    /// Writes Jellyfin's program data folder `p`: `p/data/jellyfin.db` (users, and the library in
    /// the new layout), `p/data/library.db` in the old layout, and `p/root/default/<library>/`
    /// holding one `<last component>.mblink` per path and an empty `<type>.collection`.
    pub async fn write(&self, p: &Path, layout: Layout) {
        let data = p.join("data");
        std::fs::create_dir_all(&data).unwrap();
        self.write_jellyfin_db(&data.join("jellyfin.db"), layout == Layout::New)
            .await;
        if layout == Layout::Old {
            self.write_library_db(&data.join("library.db")).await;
        }
        for lib in &self.libraries {
            let dir = p.join("root").join("default").join(lib.name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{}.collection", lib.collection_type)), "").unwrap();
            for path in &lib.paths {
                let last = path.rsplit('/').next().unwrap();
                std::fs::write(dir.join(format!("{last}.mblink")), path).unwrap();
            }
        }
    }

    /// Writes `jellyfin.db`: users with their secrets (canaries), and with `with_library` the
    /// new layout's library tables.
    pub async fn write_jellyfin_db(&self, file: &Path, with_library: bool) {
        let mut conn = create_db(file).await;
        run_all(&mut conn, USERS_DDL).await;
        for (n, u) in self.users.iter().enumerate() {
            sqlx::query(
                r#"INSERT INTO "Users" ("Id", "InternalId", "Username", "Password") VALUES (?, ?, ?, ?)"#,
            )
            .bind(upper(u.id))
            .bind(u.internal_id)
            .bind(&u.name)
            .bind(format!("{CANARY}PASSWORD-{n}"))
            .execute(&mut conn)
            .await
            .unwrap();
            // PermissionKind.IsAdministrator = 0, IsDisabled = 2.
            for (kind, value) in [(0, u.admin), (2, false)] {
                sqlx::query(
                    r#"INSERT INTO "Permissions" ("UserId", "Kind", "Value") VALUES (?, ?, ?)"#,
                )
                .bind(upper(u.id))
                .bind(kind)
                .bind(value)
                .execute(&mut conn)
                .await
                .unwrap();
            }
            sqlx::query(
                r#"INSERT INTO "Devices" ("UserId", "AccessToken", "AppName", "DeviceName", "DeviceId") VALUES (?, ?, 'Jellyfin Web', 'Firefox', ?)"#,
            )
            .bind(upper(u.id))
            .bind(format!("{CANARY}DEVICE-TOKEN-{n}"))
            .bind(format!("device-{n}"))
            .execute(&mut conn)
            .await
            .unwrap();
            sqlx::query(r#"INSERT INTO "DeviceOptions" ("DeviceId", "CustomName") VALUES (?, ?)"#)
                .bind(format!("device-{n}"))
                .bind(format!("{CANARY}DEVICE-NAME-{n}"))
                .execute(&mut conn)
                .await
                .unwrap();
        }
        sqlx::query(
            r#"INSERT INTO "ApiKeys" ("DateCreated", "Name", "AccessToken") VALUES ('2025-01-01 00:00:00', 'b20', ?)"#,
        )
        .bind(format!("{CANARY}API-KEY"))
        .execute(&mut conn)
        .await
        .unwrap();
        if with_library {
            self.write_new_library(&mut conn).await;
        }
        conn.close().await.unwrap();
    }

    async fn write_new_library(&self, conn: &mut SqliteConnection) {
        run_all(conn, NEW_LIBRARY_DDL).await;
        sqlx::query(r#"INSERT INTO "BaseItems" ("Id", "Type", "Name") VALUES (?, 'PLACEHOLDER', 'PLACEHOLDER')"#)
            .bind(upper(DETACHED))
            .execute(&mut *conn)
            .await
            .unwrap();
        let mut people: std::collections::HashMap<(&str, Option<&str>), Uuid> =
            std::collections::HashMap::new();
        for it in &self.items {
            let m = &it.meta;
            sqlx::query(
                r#"INSERT INTO "BaseItems" ("Id", "Type", "Name", "Path", "ParentId", "SeriesId", "SeasonId", "IndexNumber", "ParentIndexNumber", "ForcedSortName", "OriginalTitle", "Overview", "Tagline", "OfficialRating", "CommunityRating", "CriticRating", "PremiereDate", "EndDate", "ProductionYear", "Genres", "Studios", "Tags") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(upper(it.id))
            .bind(dotnet_type(it.kind))
            .bind(&it.name)
            .bind(it.path.as_deref())
            .bind(it.parent.map(upper))
            .bind(it.series.map(upper))
            .bind(it.season.map(upper))
            .bind(it.index)
            .bind(it.parent_index)
            .bind(m.forced_sort_name)
            .bind(m.original_title)
            .bind(m.overview)
            .bind(m.tagline)
            .bind(m.official_rating)
            .bind(m.community_rating)
            .bind(m.critic_rating)
            .bind(m.premiere_date)
            .bind(m.end_date)
            .bind(m.production_year)
            .bind(m.genres)
            .bind(m.studios)
            .bind(m.tags)
            .execute(&mut *conn)
            .await
            .unwrap();
            for img in &it.images {
                sqlx::query(
                    r#"INSERT INTO "BaseItemImageInfos" ("Id", "Path", "DateModified", "ImageType", "Width", "Height", "Blurhash", "ItemId") VALUES (?, ?, '2025-01-01 00:00:00', ?, ?, ?, ?, ?)"#,
                )
                .bind(upper(Uuid::new_v4()))
                .bind(&img.path)
                .bind(img.kind)
                .bind(img.width)
                .bind(img.height)
                .bind(img.blurhash.map(|b| b.as_bytes().to_vec()))
                .bind(upper(it.id))
                .execute(&mut *conn)
                .await
                .unwrap();
            }
            for c in &it.people {
                let person = *people.entry((c.name, c.kind)).or_insert_with(Uuid::new_v4);
                sqlx::query(
                    r#"INSERT INTO "PeopleBaseItemMap" ("SortOrder", "ListOrder", "Role", "ItemId", "PeopleId") VALUES (?, ?, ?, ?, ?)"#,
                )
                .bind(c.sort_order)
                .bind(c.list_order)
                .bind(c.role)
                .bind(upper(it.id))
                .bind(upper(person))
                .execute(&mut *conn)
                .await
                .unwrap();
            }
            for (provider, value) in &it.providers {
                sqlx::query(
                    r#"INSERT INTO "BaseItemProviders" ("ItemId", "ProviderId", "ProviderValue") VALUES (?, ?, ?)"#,
                )
                .bind(upper(it.id))
                .bind(*provider)
                .bind(value)
                .execute(&mut *conn)
                .await
                .unwrap();
            }
        }
        for ((name, kind), id) in &people {
            sqlx::query(r#"INSERT INTO "Peoples" ("Id", "Name", "PersonType") VALUES (?, ?, ?)"#)
                .bind(upper(*id))
                .bind(*name)
                .bind(*kind)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        for ud in &self.user_data {
            let key = ud
                .custom_key
                .clone()
                .unwrap_or_else(|| self.key_of(ud.item));
            sqlx::query(
                r#"INSERT INTO "UserData" ("CustomDataKey", "UserId", "ItemId", "Rating", "PlaybackPositionTicks", "PlayCount", "IsFavorite", "LastPlayedDate", "Played", "AudioStreamIndex", "SubtitleStreamIndex", "Likes") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(key)
            .bind(upper(ud.user))
            .bind(upper(ud.item))
            .bind(ud.rating)
            .bind(ud.position)
            .bind(ud.play_count)
            .bind(ud.favorite)
            .bind(ud.last_played)
            .bind(ud.played)
            .bind(ud.audio)
            .bind(ud.subtitle)
            .bind(ud.likes)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        for s in &self.streams {
            sqlx::query(
                r#"INSERT INTO "MediaStreamInfos" ("ItemId", "StreamIndex", "StreamType", "Codec", "Language", "Title", "IsExternal", "Path", "IsDefault", "IsForced") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(upper(s.item))
            .bind(s.index)
            .bind(match s.kind {
                "Audio" => 0,
                "Video" => 1,
                "Subtitle" => 2,
                _ => 4,
            })
            .bind(s.codec)
            .bind(s.language)
            .bind(s.title)
            .bind(s.external)
            .bind(s.path.as_deref())
            .bind(s.default)
            .bind(s.forced)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    }

    /// Writes the old layout's `library.db` (or `library.db.old`, by file name).
    pub async fn write_library_db(&self, file: &Path) {
        let mut conn = create_db(file).await;
        run_all(&mut conn, OLD_LIBRARY_DDL).await;
        for it in &self.items {
            let providers: Vec<String> = it
                .providers
                .iter()
                .map(|(p, v)| format!("{p}={v}"))
                .collect();
            let m = &it.meta;
            let old_date = |d: Option<&str>| {
                d.map(|d| {
                    if d.ends_with('Z') {
                        d.to_string()
                    } else {
                        format!("{d}Z")
                    }
                })
            };
            sqlx::query(
                "INSERT INTO TypedBaseItems (guid, type, Path, ParentId, Name, IndexNumber, ParentIndexNumber, IsVirtualItem, UserDataKey, SeasonId, SeriesId, ProviderIds, ForcedSortName, OriginalTitle, Overview, Tagline, OfficialRating, CommunityRating, CriticRating, PremiereDate, EndDate, ProductionYear, Genres, Studios, Tags, Images) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(it.id.to_bytes_le().to_vec())
            .bind(dotnet_type(it.kind))
            .bind(it.path.as_deref())
            .bind(it.parent.map(|p| p.to_bytes_le().to_vec()))
            .bind(&it.name)
            .bind(it.index)
            .bind(it.parent_index)
            .bind(&it.user_data_key)
            .bind(it.season.map(upper))
            .bind(it.series.map(upper))
            .bind((!providers.is_empty()).then(|| providers.join("|")))
            .bind(m.forced_sort_name)
            .bind(m.original_title)
            .bind(m.overview)
            .bind(m.tagline)
            .bind(m.official_rating)
            .bind(m.community_rating)
            .bind(m.critic_rating)
            .bind(old_date(m.premiere_date))
            .bind(old_date(m.end_date))
            .bind(m.production_year)
            .bind(m.genres)
            .bind(m.studios)
            .bind(m.tags)
            .bind(old_images(&it.images))
            .execute(&mut conn)
            .await
            .unwrap();
            for c in &it.people {
                sqlx::query(
                    "INSERT INTO People (ItemId, Name, Role, PersonType, SortOrder, ListOrder) VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(it.id.to_bytes_le().to_vec())
                .bind(c.name)
                .bind(c.role)
                .bind(c.kind)
                .bind(c.sort_order)
                .bind(c.list_order)
                .execute(&mut conn)
                .await
                .unwrap();
            }
        }
        for ud in &self.user_data {
            let user = self.users.iter().find(|u| u.id == ud.user).unwrap();
            let last = ud.last_played.map(|d| {
                if d.ends_with('Z') {
                    d.to_string()
                } else {
                    format!("{d}Z")
                }
            });
            sqlx::query(
                "INSERT INTO UserDatas (key, userId, rating, played, playCount, isFavorite, playbackPositionTicks, lastPlayedDate, AudioStreamIndex, SubtitleStreamIndex) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(self.key_of(ud.item))
            .bind(user.internal_id)
            .bind(ud.rating)
            .bind(ud.played)
            .bind(ud.play_count)
            .bind(ud.favorite)
            .bind(ud.position)
            .bind(last)
            .bind(ud.audio)
            .bind(ud.subtitle)
            .execute(&mut conn)
            .await
            .unwrap();
        }
        for s in &self.streams {
            sqlx::query(
                "INSERT INTO mediastreams (ItemId, StreamIndex, StreamType, Codec, Language, Title, IsExternal, Path, IsDefault, IsForced) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(s.item.to_bytes_le().to_vec())
            .bind(s.index)
            .bind(s.kind)
            .bind(s.codec)
            .bind(s.language)
            .bind(s.title)
            .bind(s.external)
            .bind(s.path.as_deref())
            .bind(s.default)
            .bind(s.forced)
            .execute(&mut conn)
            .await
            .unwrap();
        }
        conn.close().await.unwrap();
    }

    /// The item's `UserDataKey`, or a key no item has.
    fn key_of(&self, item: Uuid) -> String {
        self.items
            .iter()
            .find(|i| i.id == item)
            .map(|i| i.user_data_key.clone())
            .unwrap_or_else(|| format!("detached-{}", item.simple()))
    }
}

const USERS_DDL: &[&str] = &[
    r#"CREATE TABLE "Users" ("Id" TEXT NOT NULL PRIMARY KEY, "InternalId" INTEGER NOT NULL, "Username" TEXT NOT NULL, "Password" TEXT NULL, "MustUpdatePassword" INTEGER NOT NULL DEFAULT 0)"#,
    r#"CREATE TABLE "Permissions" ("Id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, "UserId" TEXT NULL, "Kind" INTEGER NOT NULL, "Value" INTEGER NOT NULL)"#,
    r#"CREATE TABLE "ApiKeys" ("Id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, "DateCreated" TEXT NOT NULL, "Name" TEXT NOT NULL, "AccessToken" TEXT NOT NULL)"#,
    r#"CREATE TABLE "Devices" ("Id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, "UserId" TEXT NOT NULL, "AccessToken" TEXT NOT NULL, "AppName" TEXT NOT NULL, "DeviceName" TEXT NOT NULL, "DeviceId" TEXT NOT NULL)"#,
    r#"CREATE TABLE "DeviceOptions" ("Id" INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, "DeviceId" TEXT NOT NULL, "CustomName" TEXT NULL)"#,
];

const NEW_LIBRARY_DDL: &[&str] = &[
    r#"CREATE TABLE "BaseItems" ("Id" TEXT NOT NULL PRIMARY KEY, "Type" TEXT NOT NULL, "Name" TEXT NULL, "Path" TEXT NULL, "ParentId" TEXT NULL, "SeriesId" TEXT NULL, "SeasonId" TEXT NULL, "IndexNumber" INTEGER NULL, "ParentIndexNumber" INTEGER NULL, "IsVirtualItem" INTEGER NOT NULL DEFAULT 0, "ForcedSortName" TEXT NULL, "OriginalTitle" TEXT NULL, "Overview" TEXT NULL, "Tagline" TEXT NULL, "OfficialRating" TEXT NULL, "CommunityRating" REAL NULL, "CriticRating" REAL NULL, "PremiereDate" TEXT NULL, "EndDate" TEXT NULL, "ProductionYear" INTEGER NULL, "Genres" TEXT NULL, "Studios" TEXT NULL, "Tags" TEXT NULL, "IsLocked" INTEGER NOT NULL DEFAULT 0)"#,
    r#"CREATE TABLE "BaseItemImageInfos" ("Id" TEXT NOT NULL PRIMARY KEY, "Path" TEXT NOT NULL, "DateModified" TEXT NULL, "ImageType" INTEGER NOT NULL, "Width" INTEGER NOT NULL, "Height" INTEGER NOT NULL, "Blurhash" BLOB NULL, "ItemId" TEXT NOT NULL)"#,
    r#"CREATE TABLE "Peoples" ("Id" TEXT NOT NULL PRIMARY KEY, "Name" TEXT NOT NULL, "PersonType" TEXT NULL)"#,
    r#"CREATE TABLE "PeopleBaseItemMap" ("SortOrder" INTEGER NULL, "ListOrder" INTEGER NULL, "Role" TEXT NOT NULL, "ItemId" TEXT NOT NULL, "PeopleId" TEXT NOT NULL, PRIMARY KEY ("ItemId", "PeopleId", "Role"))"#,
    r#"CREATE TABLE "BaseItemProviders" ("ItemId" TEXT NOT NULL, "ProviderId" TEXT NOT NULL, "ProviderValue" TEXT NOT NULL, PRIMARY KEY ("ItemId", "ProviderId"))"#,
    r#"CREATE TABLE "UserData" ("CustomDataKey" TEXT NOT NULL, "UserId" TEXT NOT NULL, "ItemId" TEXT NOT NULL, "Rating" REAL NULL, "PlaybackPositionTicks" INTEGER NOT NULL, "PlayCount" INTEGER NOT NULL, "IsFavorite" INTEGER NOT NULL, "LastPlayedDate" TEXT NULL, "Played" INTEGER NOT NULL, "AudioStreamIndex" INTEGER NULL, "SubtitleStreamIndex" INTEGER NULL, "Likes" INTEGER NULL, PRIMARY KEY ("ItemId", "UserId", "CustomDataKey"))"#,
    r#"CREATE TABLE "MediaStreamInfos" ("ItemId" TEXT NOT NULL, "StreamIndex" INTEGER NOT NULL, "StreamType" INTEGER NOT NULL, "Codec" TEXT NULL, "Language" TEXT NULL, "Title" TEXT NULL, "IsExternal" INTEGER NOT NULL, "Path" TEXT NULL, "IsDefault" INTEGER NOT NULL, "IsForced" INTEGER NOT NULL, PRIMARY KEY ("ItemId", "StreamIndex"))"#,
];

const OLD_LIBRARY_DDL: &[&str] = &[
    "CREATE TABLE TypedBaseItems (guid GUID primary key NOT NULL, type TEXT NOT NULL, Path TEXT, ParentId GUID, Name Text, IndexNumber INT, ParentIndexNumber INT, IsVirtualItem BIT, UserDataKey Text, SeasonId GUID, SeriesId GUID, ProviderIds Text, ForcedSortName Text, OriginalTitle Text, Overview Text, Tagline Text, OfficialRating Text, CommunityRating Float, CriticRating Float, PremiereDate DATETIME, EndDate DATETIME, ProductionYear INT, Genres Text, Studios Text, Tags Text, IsLocked BIT, LockedFields Text, Images Text)",
    "CREATE TABLE People (ItemId GUID, Name TEXT NOT NULL, Role TEXT, PersonType TEXT, SortOrder int, ListOrder int)",
    "CREATE TABLE UserDatas (key nvarchar not null, userId INT not null, rating float null, played bit not null, playCount int not null, isFavorite bit not null, playbackPositionTicks bigint not null, lastPlayedDate datetime null, AudioStreamIndex INT, SubtitleStreamIndex INT)",
    "CREATE UNIQUE INDEX UserDatasIndex1 ON UserDatas (key, userId)",
    "CREATE TABLE mediastreams (ItemId GUID, StreamIndex INT, StreamType TEXT, Codec TEXT, Language TEXT, Title TEXT, IsExternal BIT, Path TEXT, IsDefault BIT, IsForced BIT, PRIMARY KEY (ItemId, StreamIndex))",
];

fn upper(id: Uuid) -> String {
    id.hyphenated().to_string().to_uppercase()
}

fn dotnet_type(kind: &str) -> String {
    let ns = match kind {
        "Movie" => "Movies.",
        "Series" | "Season" | "Episode" => "TV.",
        "Audio" | "MusicAlbum" | "MusicArtist" => "Audio.",
        _ => "",
    };
    format!("MediaBrowser.Controller.Entities.{ns}{kind}")
}

/// A new database in rollback-journal mode, so no `-wal` file stays behind.
async fn create_db(file: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(file)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .connect()
        .await
        .unwrap()
}

async fn run_all(conn: &mut SqliteConnection, statements: &[&'static str]) {
    for sql in statements {
        sqlx::query(*sql).execute(&mut *conn).await.unwrap();
    }
}

/// One line per file and folder under `root` (and `root` itself): relative path, size, mtime.
pub fn listing(root: &Path) -> Vec<String> {
    fn line(rel: &str, meta: &std::fs::Metadata) -> String {
        let mtime = meta
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let size = if meta.is_dir() { 0 } else { meta.len() };
        format!("{rel} {size} {mtime}")
    }
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            out.push(line(
                &path.strip_prefix(root).unwrap().display().to_string(),
                &meta,
            ));
            if meta.is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = vec![line(".", &std::fs::metadata(root).unwrap())];
    walk(root, root, &mut out);
    out.sort();
    out
}

/// True when any of `files` that exist contains a canary.
pub fn files_hold_canary(files: &[PathBuf]) -> bool {
    files.iter().filter(|f| f.exists()).any(|f| {
        let bytes = std::fs::read(f).unwrap();
        bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes())
    })
}

type LogBuf = Arc<Mutex<Vec<u8>>>;

thread_local! {
    /// The buffer of this thread's innermost live `LogCapture`.
    static CAPTURE: RefCell<Option<LogBuf>> = const { RefCell::new(None) };
}

/// Captures every log event of the current thread (TRACE and up) while it lives. Tests on
/// tokio's default current-thread runtime see the events of the tasks they spawn too.
///
/// One global subscriber, installed by the first capture, writes each event to its thread's
/// capture. A scoped subscriber per capture (`set_default`) would be flaky: tracing caches each
/// callsite's interest globally, so a callsite first hit while another test installs its
/// subscriber can stay disabled for every subscriber. The global subscriber's filter is dynamic:
/// every callsite stays "sometimes", and each event checks for a capture on its own thread (a
/// `filter_fn` would cache its first answer per callsite).
pub struct LogCapture {
    buf: LogBuf,
    outer: Option<LogBuf>,
}

impl LogCapture {
    pub fn start() -> Self {
        use tracing_subscriber::Layer as _;
        use tracing_subscriber::layer::SubscriberExt as _;

        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(|| ThreadSink)
                .with_filter(tracing_subscriber::filter::dynamic_filter_fn(|_, _| {
                    CAPTURE.with(|c| c.borrow().is_some())
                }));
            tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer))
                .expect("no other global subscriber in the API tests");
        });
        // Callsites first hit before or during the install are enabled again.
        tracing::callsite::rebuild_interest_cache();
        let buf = LogBuf::default();
        let outer = CAPTURE.with(|c| c.replace(Some(buf.clone())));
        Self { buf, outer }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().unwrap()).into_owned()
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        let outer = self.outer.take();
        CAPTURE.with(|c| *c.borrow_mut() = outer);
    }
}

/// Writes to the current thread's capture.
struct ThreadSink;

impl std::io::Write for ThreadSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        CAPTURE.with(|c| {
            if let Some(buf) = c.borrow().as_ref() {
                buf.lock().unwrap().extend_from_slice(bytes);
            }
        });
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
