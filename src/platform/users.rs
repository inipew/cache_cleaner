use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AndroidUser {
    pub user_id: u32,
    pub ce_path: PathBuf,
    pub de_path: PathBuf,
    pub media_path: PathBuf,
}

pub fn enumerate_users() -> Vec<AndroidUser> {
    let mut users = Vec::new();
    let mut user_ids = vec![0u32];

    // 1. Check /data/system/users/
    if let Ok(entries) = fs::read_dir("/data/system/users") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(num_str) = file_name.strip_suffix(".xml") {
                    if let Ok(id) = num_str.parse::<u32>() {
                        if !user_ids.contains(&id) {
                            user_ids.push(id);
                        }
                    }
                }
            }
        }
    }

    // 2. Check /data/user/
    if let Ok(entries) = fs::read_dir("/data/user") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse::<u32>() {
                    if !user_ids.contains(&id) {
                        user_ids.push(id);
                    }
                }
            }
        }
    }

    // 3. Check /data/user_de/
    if let Ok(entries) = fs::read_dir("/data/user_de") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse::<u32>() {
                    if !user_ids.contains(&id) {
                        user_ids.push(id);
                    }
                }
            }
        }
    }

    // 4. Check /data/media/ (External storage per user)
    if let Ok(entries) = fs::read_dir("/data/media") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse::<u32>() {
                    if !user_ids.contains(&id) {
                        user_ids.push(id);
                    }
                }
            }
        }
    }

    // 5. Check /data/misc/profiles/cur/ (ART profiles per user)
    if let Ok(entries) = fs::read_dir("/data/misc/profiles/cur") {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse::<u32>() {
                    if !user_ids.contains(&id) {
                        user_ids.push(id);
                    }
                }
            }
        }
    }

    user_ids.sort_unstable();
    user_ids.dedup();

    for id in user_ids {
        // Canonical CE path: User 0 canonical is /data/data (since /data/user/0 is a symlink on Android)
        let ce_path = if id == 0 {
            let primary = PathBuf::from("/data/data");
            if primary.exists() {
                primary
            } else {
                PathBuf::from("/data/user/0")
            }
        } else {
            let user_ce = PathBuf::from(format!("/data/user/{}", id));
            fs::canonicalize(&user_ce).unwrap_or(user_ce)
        };

        let de_raw = PathBuf::from(format!("/data/user_de/{}", id));
        let de_path = fs::canonicalize(&de_raw).unwrap_or(de_raw);

        let media_raw = PathBuf::from(format!("/data/media/{}", id));
        let media_path = fs::canonicalize(&media_raw).unwrap_or(media_raw);

        users.push(AndroidUser {
            user_id: id,
            ce_path,
            de_path,
            media_path,
        });
    }

    users
}
