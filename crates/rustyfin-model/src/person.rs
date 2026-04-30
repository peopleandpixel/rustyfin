use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NameIdPair {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PersonDto {
    pub name: String,
    pub id: String,
    pub role: Option<String>,
    #[serde(rename = "Type")]
    pub person_type: Option<String>,
    pub movie_count: i64,
    pub series_count: i64,
}
