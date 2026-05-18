use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFile {
    vars: BTreeMap<String, String>,
}

impl EnvFile {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.vars.get(key)
    }

    pub fn required(&self, key: &str) -> Result<String, String> {
        match self.vars.get(key) {
            Some(value) if !value.trim().is_empty() => Ok(value.clone()),
            Some(_) => Err(format!("{key} must not be empty in secrets.env")),
            None => Err(format!("{key} is missing from secrets.env")),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.vars.iter()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.vars.keys()
    }

    pub fn secret_values(&self) -> impl Iterator<Item = &String> {
        self.vars.values().filter(|value| !value.is_empty())
    }

    pub fn insert(&mut self, key: String, value: String) -> Result<(), String> {
        validate_key(&key)?;
        validate_value(&value, &key)?;
        if self.vars.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate env key `{key}`"));
        }
        Ok(())
    }
}

pub fn read_env_file(path: impl AsRef<Path>) -> Result<EnvFile, String> {
    let path = path.as_ref();
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read env file {}: {error}", path.display()))?;
    parse_env_file_str(&raw)
        .map_err(|error| format!("invalid env file {}: {error}", path.display()))
}

pub fn parse_env_file_str(raw: &str) -> Result<EnvFile, String> {
    let mut file = EnvFile::default();
    for (index, original_line) in raw.lines().enumerate() {
        let line_number = index + 1;
        let line = original_line.strip_suffix('\r').unwrap_or(original_line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.trim() != line {
            return Err(format!(
                "line {line_number}: env entries must not have leading or trailing whitespace"
            ));
        }
        if line.starts_with("export ") {
            return Err(format!(
                "line {line_number}: shell `export` syntax is not supported"
            ));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {line_number}: expected KEY=value"));
        };
        validate_key(key).map_err(|error| format!("line {line_number}: {error}"))?;
        validate_value(value, key).map_err(|error| format!("line {line_number}: {error}"))?;
        if file
            .vars
            .insert(key.to_string(), value.to_string())
            .is_some()
        {
            return Err(format!("line {line_number}: duplicate env key `{key}`"));
        }
    }
    Ok(file)
}

fn validate_key(key: &str) -> Result<(), String> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err("env key must not be empty".to_string());
    };
    if !(first == '_' || first.is_ascii_uppercase()) {
        return Err(format!(
            "env key `{key}` must start with an uppercase ASCII letter or underscore"
        ));
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit()) {
        return Err(format!(
            "env key `{key}` may only contain uppercase ASCII letters, digits, and underscores"
        ));
    }
    Ok(())
}

fn validate_value(value: &str, key: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("env value for `{key}` must not be empty"));
    }
    if value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(format!(
            "env value for `{key}` must not contain whitespace in secrets.env"
        ));
    }
    if value.contains('$') || value.contains('`') {
        return Err(format!(
            "env value for `{key}` must not contain shell expansion syntax"
        ));
    }
    if value.contains('#') {
        return Err(format!(
            "env value for `{key}` must not contain shell comment syntax"
        ));
    }
    if value.contains('\0') {
        return Err(format!("env value for `{key}` must not contain NUL bytes"));
    }
    Ok(())
}
