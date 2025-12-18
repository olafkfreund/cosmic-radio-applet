use serde::{Deserialize, Serialize};
use reqwest::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Station {
    pub stationuuid: String,
    pub name: String,
    pub url: String,
    pub url_resolved: String,
    pub homepage: String,
    pub favicon: String,
    pub tags: String,
    pub country: String,
    pub language: String,
}

pub async fn search_stations(query: String) -> Result<Vec<Station>, Error> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    
    let client = reqwest::Client::new();
    let url = format!("https://de1.api.radio-browser.info/json/stations/search?name={}&limit=20", query);
    
    let response = client.get(&url)
        .send()
        .await?;
        
    let stations: Vec<Station> = response.json().await?;
    Ok(stations)
}
