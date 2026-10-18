//! Land-cover classes and their appearance in Minecraft.
//!
//! Every source class gets a key: `veg:<凡例コード>` (環境省 1/25,000 現存植生図),
//! `jaxa:<n>` (JAXA 高解像度土地利用土地被覆図) or `osm:<tag>` (OpenStreetMap
//! land use). `config/landcover_map.csv` maps each key to blocks, a planting
//! pattern, trees and a biome. Missing rows are generated from the rules
//! below and appended, so the table can be edited and re-used.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub key: String,
    pub name: String,
    /// forest / plantation / bamboo / shrub / grass / alpine / farm / paddy / tea / orchard /
    /// greenhouse / solar / urban / residential / industrial / park / cemetery / golf / airport /
    /// port / bare / sand / rock / wetland / water / quarry / parking / sports
    pub category: String,
    pub surface: String,
    pub subsurface: String,
    /// none | paddy | field | tea_rows | orchard | greenhouse | solar | cemetery | runway | apron | parking | sports | golf
    pub pattern: String,
    /// species:weight list, e.g. "cedar:6,cypress:4"
    pub trees: String,
    /// trees per 100 m²
    pub density: f32,
    /// default height range (m) "min-max"
    pub height: String,
    /// plant:probability list, e.g. "short_grass:0.3,fern:0.1"
    pub ground: String,
    pub biome: String,
}

pub const HEADER: &str = "key,name,category,surface,subsurface,pattern,trees,tree_density_per_100m2,tree_height_m,groundcover,biome";

impl Spec {
    fn to_csv(&self) -> String {
        let q = |s: &str| if s.contains(',') || s.contains('"') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        format!(
            "{},{},{},{},{},{},{},{},{},{},{}",
            q(&self.key),
            q(&self.name),
            q(&self.category),
            q(&self.surface),
            q(&self.subsurface),
            q(&self.pattern),
            q(&self.trees),
            self.density,
            q(&self.height),
            q(&self.ground),
            q(&self.biome)
        )
    }

    /// (min, max) tree height in metres.
    pub fn height_range(&self) -> (f32, f32) {
        let mut it = self.height.split('-').filter_map(|v| v.trim().parse::<f32>().ok());
        let a = it.next().unwrap_or(0.0);
        let b = it.next().unwrap_or(a);
        (a, b)
    }

    pub fn tree_mix(&self) -> Vec<(String, f32)> {
        weighted(&self.trees)
    }

    pub fn ground_mix(&self) -> Vec<(String, f32)> {
        weighted(&self.ground)
    }
}

fn weighted(s: &str) -> Vec<(String, f32)> {
    s.split([',', ';'])
        .filter_map(|kv| {
            let kv = kv.trim();
            if kv.is_empty() {
                return None;
            }
            let (k, v) = kv.rsplit_once(':').unwrap_or((kv, "1"));
            Some((k.trim().to_string(), v.trim().parse().unwrap_or(1.0)))
        })
        .collect()
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => q = !q,
            ',' if !q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn spec(key: &str, name: &str, cat: &str, surface: &str, pattern: &str, trees: &str, density: f32, height: &str, ground: &str, biome: &str) -> Spec {
    let sub = match surface {
        "grass_block" | "podzol" | "coarse_dirt" | "moss_block" | "farmland[moisture=7]" | "rooted_dirt" | "mud" | "dirt_path" => "dirt",
        "sand" => "sand",
        "gravel" => "gravel",
        _ => "stone",
    };
    Spec {
        key: key.into(),
        name: name.into(),
        category: cat.into(),
        surface: surface.into(),
        subsurface: sub.into(),
        pattern: pattern.into(),
        trees: trees.into(),
        density,
        height: height.into(),
        ground: ground.into(),
        biome: biome.into(),
    }
}

/// Default appearance of a 現存植生図 legend class from its name (and 植生区分).
pub fn veg_default(code: &str, name: &str, group: &str) -> Spec {
    let k = format!("veg:{code}");
    let has = |w: &[&str]| w.iter().any(|x| name.contains(x));
    let g_forest = "short_grass:0.05,fern:0.08";
    // cultivated / artificial classes first
    if name.contains("茶畑") {
        return spec(&k, name, "tea", "grass_block", "tea_rows", "", 0.0, "1-1", "", "minecraft:forest");
    }
    if has(&["常緑果樹園"]) {
        return spec(&k, name, "orchard", "grass_block", "orchard", "citrus:1", 0.0, "3-4", "short_grass:0.2", "minecraft:forest");
    }
    if has(&["果樹園", "桑園", "クワ畑"]) {
        return spec(&k, name, "orchard", "grass_block", "orchard", "fruit:1", 0.0, "3-5", "short_grass:0.2", "minecraft:forest");
    }
    if has(&["放棄水田"]) {
        return spec(&k, name, "wetland", "grass_block", "none", "", 0.0, "", "tall_grass:0.35,short_grass:0.4", "minecraft:swamp");
    }
    if has(&["水田"]) {
        return spec(&k, name, "paddy", "mud", "paddy", "", 0.0, "", "", "minecraft:swamp");
    }
    if has(&["放棄畑"]) {
        return spec(&k, name, "grass", "grass_block", "none", "shrub:1", 0.3, "1-3", "tall_grass:0.3,short_grass:0.4", "minecraft:plains");
    }
    if has(&["畑", "耕作地"]) {
        return spec(&k, name, "farm", "farmland[moisture=7]", "field", "", 0.0, "", "", "minecraft:plains");
    }
    if has(&["牧草地"]) {
        return spec(&k, name, "grass", "grass_block", "none", "", 0.0, "", "short_grass:0.25", "minecraft:meadow");
    }
    if has(&["ゴルフ場", "芝地"]) {
        return spec(&k, name, "golf", "grass_block", "golf", "broadleaf:1", 0.15, "8-14", "", "minecraft:meadow");
    }
    if has(&["緑の多い住宅地"]) {
        return spec(&k, name, "residential", "grass_block", "none", "broadleaf:3,evergreen_broadleaf:2,cherry:1", 0.6, "5-10", "short_grass:0.15", "minecraft:plains");
    }
    if has(&["市街地"]) {
        return spec(&k, name, "urban", "grass_block", "none", "broadleaf:1", 0.15, "5-9", "short_grass:0.05", "minecraft:plains");
    }
    if has(&["工場地帯"]) {
        return spec(&k, name, "industrial", "gravel", "none", "", 0.0, "", "", "minecraft:plains");
    }
    if has(&["造成地"]) {
        return spec(&k, name, "bare", "coarse_dirt", "none", "", 0.0, "", "short_grass:0.05", "minecraft:plains");
    }
    if has(&["公園", "墓地"]) {
        return spec(&k, name, "park", "grass_block", "none", "broadleaf:3,evergreen_broadleaf:2,cherry:2", 1.2, "6-14", "short_grass:0.2", "minecraft:plains");
    }
    if has(&["植栽樹群", "残存"]) {
        return spec(&k, name, "park", "grass_block", "none", "broadleaf:3,evergreen_broadleaf:2", 3.0, "8-16", "short_grass:0.15", "minecraft:forest");
    }
    if has(&["開放水域"]) {
        return spec(&k, name, "water", "sand", "none", "", 0.0, "", "", "minecraft:river");
    }
    if has(&["自然裸地"]) {
        return spec(&k, name, "bare", "gravel", "none", "", 0.0, "", "", "minecraft:plains");
    }
    if has(&["砂丘"]) {
        return spec(&k, name, "sand", "sand", "none", "", 0.0, "", "short_grass:0.05", "minecraft:beach");
    }
    if has(&["岩壁", "岩角地"]) {
        return spec(&k, name, "rock", "stone", "none", "shrub:1", 0.3, "1-3", "", "minecraft:windswept_hills");
    }
    // vegetation
    if has(&["竹林", "モウソウチク", "マダケ", "ハチク"]) {
        return spec(&k, name, "bamboo", "podzol", "none", "bamboo:1", 30.0, "10-16", "", "minecraft:bamboo_jungle");
    }
    if has(&["ハイマツ"]) {
        return spec(&k, name, "alpine", "gravel", "none", "dwarf_pine:1", 25.0, "1-2", "", "minecraft:stony_peaks");
    }
    if has(&["高山", "風衝", "ハイデ", "雪田", "コマクサ"]) || group.contains("高山帯") {
        return spec(&k, name, "alpine", "gravel", "none", "", 0.0, "", "short_grass:0.15,dandelion:0.01", "minecraft:stony_peaks");
    }
    if has(&["スギ", "ヒノキ", "サワラ"]) && name.contains("植林") {
        return spec(&k, name, "plantation", "podzol", "none", "cedar:6,cypress:4", 7.0, "18-30", "fern:0.10", "minecraft:old_growth_spruce_taiga");
    }
    if has(&["カラマツ"]) {
        return spec(&k, name, "plantation", "grass_block", "none", "larch:1", 5.0, "15-25", "short_grass:0.15,fern:0.05", "minecraft:taiga");
    }
    if has(&["アカマツ", "クロマツ", "マツ植林", "リュウキュウマツ"]) {
        return spec(&k, name, "forest", "coarse_dirt", "none", "pine:7,broadleaf:3", 4.5, "12-20", "short_grass:0.1", "minecraft:forest");
    }
    if has(&["シラビソ", "オオシラビソ", "トウヒ", "コメツガ", "ツガ", "モミ", "ウラジロモミ"]) {
        return spec(&k, name, "forest", "podzol", "none", "fir:6,spruce:4", 6.0, "12-25", "fern:0.12", "minecraft:old_growth_spruce_taiga");
    }
    if has(&["ヒノキ"]) {
        return spec(&k, name, "forest", "podzol", "none", "cypress:7,broadleaf:3", 6.0, "15-25", "fern:0.08", "minecraft:taiga");
    }
    if has(&["ダケカンバ", "シラカンバ", "ウダイカンバ", "カンバ"]) {
        return spec(&k, name, "forest", "grass_block", "none", "birch:7,fir:3", 5.0, "8-18", "short_grass:0.2,fern:0.05", "minecraft:birch_forest");
    }
    if has(&["ブナ", "イヌブナ"]) {
        return spec(&k, name, "forest", "grass_block", "none", "beech:7,broadleaf:3", 4.5, "15-25", g_forest, "minecraft:forest");
    }
    if has(&["シイ", "カシ", "タブ", "クス", "ヤブツバキ", "照葉", "ウバメガシ", "マテバシイ", "ヤブニッケイ", "シキミ"]) && !has(&["ササ"]) {
        return spec(&k, name, "forest", "grass_block", "none", "evergreen_broadleaf:8,broadleaf:2", 5.0, "10-20", "fern:0.06", "minecraft:dark_forest");
    }
    if has(&["ササ", "ネザサ", "メダケ", "スズタケ", "ハコネダケ"]) && !has(&["ブナ", "ミズナラ", "コナラ", "ダケカンバ"]) {
        return spec(&k, name, "shrub", "grass_block", "none", "", 0.0, "", "fern:0.55,large_fern:0.15,short_grass:0.1", "minecraft:forest");
    }
    if has(&["ススキ", "チガヤ", "シバ群落", "草原", "ヨモギ", "イタドリ", "クズ", "外来草本", "雑草"]) {
        return spec(&k, name, "grass", "grass_block", "none", "", 0.0, "", "tall_grass:0.35,short_grass:0.35", "minecraft:plains");
    }
    if has(&["ヨシ", "オギ", "ツルヨシ", "湿原", "湿地", "マコモ", "ガマ", "ヒルムシロ"]) || group.contains("河辺") {
        return spec(&k, name, "wetland", "grass_block", "none", "willow:1", 0.2, "6-12", "tall_grass:0.55,short_grass:0.2", "minecraft:swamp");
    }
    if has(&["ヤナギ"]) {
        return spec(&k, name, "forest", "grass_block", "none", "willow:1", 3.0, "6-14", "tall_grass:0.2", "minecraft:swamp");
    }
    if has(&["伐採跡地", "低木", "ヤシャブシ", "アカメガシワ", "ウツギ", "ノリウツギ", "ツツジ群落"]) {
        return spec(&k, name, "shrub", "grass_block", "none", "shrub:6,broadleaf:2", 3.0, "2-7", "tall_grass:0.25,short_grass:0.2", "minecraft:forest");
    }
    if has(&["植林"]) {
        if has(&["常緑針葉"]) {
            return spec(&k, name, "plantation", "podzol", "none", "cedar:5,cypress:5", 6.0, "15-25", "fern:0.08", "minecraft:taiga");
        }
        return spec(&k, name, "plantation", "grass_block", "none", "broadleaf:1", 5.0, "10-18", g_forest, "minecraft:forest");
    }
    if has(&["ミズナラ", "コナラ", "クヌギ", "クリ", "シデ", "ケヤキ", "カエデ", "モミジ", "サワグルミ", "シオジ", "トチノキ", "ハンノキ", "オニグルミ", "ホオノキ", "ニセアカシア", "フサザクラ", "ムクノキ", "エノキ", "落葉広葉樹", "二次林", "サクラ"]) {
        return spec(&k, name, "forest", "grass_block", "none", "broadleaf:8,evergreen_broadleaf:1,cherry:1", 4.5, "10-20", g_forest, "minecraft:forest");
    }
    // anything else with forest-ish group names
    if group.contains("自然植生") || group.contains("代償植生") {
        return spec(&k, name, "forest", "grass_block", "none", "broadleaf:6,evergreen_broadleaf:2,fir:2", 4.0, "8-18", g_forest, "minecraft:forest");
    }
    spec(&k, name, "grass", "grass_block", "none", "", 0.0, "", "short_grass:0.2", "minecraft:plains")
}

/// JAXA classes (v25.04). Names per the product description; used where the
/// vegetation map has no polygon and for classes it lacks (solar, greenhouse).
pub fn jaxa_default(n: u8) -> Spec {
    let k = format!("jaxa:{n}");
    match n {
        1 => spec(&k, "水域", "water", "sand", "none", "", 0.0, "", "", "minecraft:river"),
        2 => spec(&k, "都市", "urban", "grass_block", "none", "broadleaf:1", 0.1, "5-9", "short_grass:0.05", "minecraft:plains"),
        3 => spec(&k, "水田", "paddy", "mud", "paddy", "", 0.0, "", "", "minecraft:swamp"),
        4 => spec(&k, "畑地", "farm", "farmland[moisture=7]", "field", "", 0.0, "", "", "minecraft:plains"),
        5 => spec(&k, "草地", "grass", "grass_block", "none", "", 0.0, "", "tall_grass:0.2,short_grass:0.35", "minecraft:plains"),
        6 => spec(&k, "落葉広葉樹", "forest", "grass_block", "none", "broadleaf:1", 4.5, "10-20", "short_grass:0.05,fern:0.08", "minecraft:forest"),
        7 => spec(&k, "落葉針葉樹", "forest", "grass_block", "none", "larch:1", 5.0, "15-25", "short_grass:0.15", "minecraft:taiga"),
        8 => spec(&k, "常緑広葉樹", "forest", "grass_block", "none", "evergreen_broadleaf:1", 5.0, "10-20", "fern:0.06", "minecraft:dark_forest"),
        9 => spec(&k, "常緑針葉樹", "forest", "podzol", "none", "cedar:5,cypress:5", 6.0, "15-28", "fern:0.10", "minecraft:old_growth_spruce_taiga"),
        10 => spec(&k, "裸地", "bare", "gravel", "none", "", 0.0, "", "", "minecraft:plains"),
        11 => spec(&k, "竹林", "bamboo", "podzol", "none", "bamboo:1", 30.0, "10-16", "", "minecraft:bamboo_jungle"),
        12 => spec(&k, "ソーラーパネル", "solar", "gravel", "solar", "", 0.0, "", "", "minecraft:plains"),
        13 => spec(&k, "湿地", "wetland", "grass_block", "none", "", 0.0, "", "tall_grass:0.5,short_grass:0.2", "minecraft:swamp"),
        14 => spec(&k, "温室", "greenhouse", "grass_block", "greenhouse", "", 0.0, "", "", "minecraft:plains"),
        15 => spec(&k, "その他", "grass", "grass_block", "none", "", 0.0, "", "short_grass:0.2", "minecraft:plains"),
        _ => spec(&k, "不明", "grass", "grass_block", "none", "", 0.0, "", "short_grass:0.2", "minecraft:plains"),
    }
}

/// OSM land-use keys we recognise (tag → appearance). Checked in this order
/// against a feature's tags; the first match wins.
pub fn osm_defaults() -> Vec<Spec> {
    vec![
        spec("osm:crop=tea", "茶畑 (OSM)", "tea", "grass_block", "tea_rows", "", 0.0, "1-1", "", "minecraft:forest"),
        spec("osm:landuse=greenhouse_horticulture", "温室 (OSM)", "greenhouse", "grass_block", "greenhouse", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:building=greenhouse", "温室 (OSM)", "greenhouse", "grass_block", "greenhouse", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:plant:source=solar", "太陽光発電所 (OSM)", "solar", "gravel", "solar", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:generator:source=solar", "太陽光パネル (OSM)", "solar", "gravel", "solar", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:aeroway=runway", "滑走路 (OSM)", "airport", "gray_concrete", "runway", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:aeroway=taxiway", "誘導路 (OSM)", "airport", "gray_concrete", "apron", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:aeroway=apron", "エプロン (OSM)", "airport", "light_gray_concrete", "apron", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:aeroway=helipad", "ヘリパッド (OSM)", "airport", "light_gray_concrete", "apron", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:aeroway=aerodrome", "空港敷地 (OSM)", "airport", "grass_block", "none", "", 0.0, "", "short_grass:0.05", "minecraft:plains"),
        spec("osm:landuse=cemetery", "墓地 (OSM)", "cemetery", "gravel", "cemetery", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:amenity=grave_yard", "墓地 (OSM)", "cemetery", "gravel", "cemetery", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:leisure=golf_course", "ゴルフ場 (OSM)", "golf", "grass_block", "golf", "broadleaf:1", 0.15, "8-14", "", "minecraft:meadow"),
        spec("osm:leisure=pitch", "競技場 (OSM)", "sports", "grass_block", "sports", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:leisure=track", "トラック (OSM)", "sports", "red_terracotta", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:leisure=stadium", "スタジアム (OSM)", "sports", "grass_block", "sports", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:leisure=park", "公園 (OSM)", "park", "grass_block", "none", "broadleaf:3,evergreen_broadleaf:2,cherry:2", 1.2, "6-14", "short_grass:0.2,poppy:0.005,dandelion:0.005", "minecraft:plains"),
        spec("osm:leisure=garden", "庭園 (OSM)", "park", "grass_block", "none", "broadleaf:2,evergreen_broadleaf:2,pine:1,cherry:1", 1.5, "4-10", "short_grass:0.2", "minecraft:plains"),
        spec("osm:leisure=playground", "遊び場 (OSM)", "park", "sand", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:amenity=parking", "駐車場 (OSM)", "parking", "gray_concrete", "parking", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=quarry", "採石場 (OSM)", "quarry", "stone", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=landfill", "処分場 (OSM)", "bare", "coarse_dirt", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=construction", "工事中 (OSM)", "bare", "coarse_dirt", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=railway", "鉄道用地 (OSM)", "industrial", "gravel", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:industrial=port", "港湾 (OSM)", "port", "polished_andesite", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=port", "港湾 (OSM)", "port", "polished_andesite", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=harbour", "港湾 (OSM)", "port", "polished_andesite", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=orchard", "果樹園 (OSM)", "orchard", "grass_block", "orchard", "fruit:1", 0.0, "3-5", "short_grass:0.2", "minecraft:forest"),
        spec("osm:landuse=vineyard", "ぶどう園 (OSM)", "orchard", "grass_block", "orchard", "fruit:1", 0.0, "2-3", "short_grass:0.2", "minecraft:forest"),
        spec("osm:landuse=paddy", "水田 (OSM)", "paddy", "mud", "paddy", "", 0.0, "", "", "minecraft:swamp"),
        spec("osm:crop=rice", "水田 (OSM)", "paddy", "mud", "paddy", "", 0.0, "", "", "minecraft:swamp"),
        spec("osm:landuse=farmland", "農地 (OSM)", "farm", "farmland[moisture=7]", "field", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=industrial", "工業地 (OSM)", "industrial", "gravel", "none", "", 0.0, "", "", "minecraft:plains"),
        spec("osm:landuse=commercial", "商業地 (OSM)", "urban", "grass_block", "none", "broadleaf:1", 0.1, "5-9", "", "minecraft:plains"),
        spec("osm:landuse=retail", "商業地 (OSM)", "urban", "grass_block", "none", "broadleaf:1", 0.1, "5-9", "", "minecraft:plains"),
        spec("osm:landuse=military", "防衛施設 (OSM)", "urban", "grass_block", "none", "", 0.0, "", "short_grass:0.1", "minecraft:plains"),
        spec("osm:natural=beach", "砂浜 (OSM)", "sand", "sand", "none", "", 0.0, "", "", "minecraft:beach"),
        spec("osm:natural=bare_rock", "岩場 (OSM)", "rock", "stone", "none", "", 0.0, "", "", "minecraft:stony_peaks"),
        spec("osm:natural=scree", "がれ場 (OSM)", "rock", "gravel", "none", "", 0.0, "", "", "minecraft:stony_peaks"),
        spec("osm:man_made=wastewater_plant", "下水処理場 (OSM)", "industrial", "gray_concrete", "none", "", 0.0, "", "", "minecraft:plains"),
    ]
}

/// Load the mapping table, adding default rows for keys it does not have
/// yet (rewrites the file when rows were added). `veg_legend` rows:
/// (code, name, group).
pub fn load_or_create(path: &Path, veg_legend: &[(String, String, String)]) -> Result<BTreeMap<String, Spec>> {
    let mut map: BTreeMap<String, Spec> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    if path.exists() {
        let s = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
        for line in s.lines().skip(1) {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let c = split_csv(line);
            if c.len() < 11 {
                continue;
            }
            let sp = Spec {
                key: c[0].clone(),
                name: c[1].clone(),
                category: c[2].clone(),
                surface: c[3].clone(),
                subsurface: c[4].clone(),
                pattern: c[5].clone(),
                trees: c[6].clone(),
                density: c[7].parse().unwrap_or(0.0),
                height: c[8].clone(),
                ground: c[9].clone(),
                biome: c[10].clone(),
            };
            order.push(sp.key.clone());
            map.insert(sp.key.clone(), sp);
        }
    }
    let mut added = 0;
    let mut add = |sp: Spec, map: &mut BTreeMap<String, Spec>, order: &mut Vec<String>| {
        if !map.contains_key(&sp.key) {
            order.push(sp.key.clone());
            map.insert(sp.key.clone(), sp);
            added += 1;
        }
    };
    for (code, name, group) in veg_legend {
        add(veg_default(code, name, group), &mut map, &mut order);
    }
    for n in 1..=15u8 {
        add(jaxa_default(n), &mut map, &mut order);
    }
    for sp in osm_defaults() {
        add(sp, &mut map, &mut order);
    }
    if added > 0 {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        // BOM so that Excel opens the UTF-8 CSV correctly
        f.write_all("\u{feff}".as_bytes())?;
        writeln!(f, "{HEADER}")?;
        for k in &order {
            writeln!(f, "{}", map[k].to_csv())?;
        }
    }
    Ok(map)
}

/// Classes in the table's row order (new rows are appended at the end, so a
/// class keeps its index — the value stored in the lc rasters — when regions
/// with new legend codes are added).
pub fn classes(path: &Path, veg_legend: &[(String, String, String)]) -> Result<Vec<Spec>> {
    let map = load_or_create(path, veg_legend)?;
    let s = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    let mut seen = std::collections::HashSet::new();
    Ok(s.lines()
        .skip(1)
        .filter_map(|l| {
            let k = split_csv(l).into_iter().next()?;
            (seen.insert(k.clone())).then(|| map.get(&k).cloned()).flatten()
        })
        .collect())
}

/// Read `veg_legend.csv` written by the `vegmap` command: (code, name, group).
pub fn read_veg_legend(path: &Path) -> Result<Vec<(String, String, String)>> {
    let s = std::fs::read_to_string(path).with_context(|| format!("{} (run `jpdem vegmap` first)", path.display()))?;
    Ok(s.lines()
        .skip(1)
        .filter_map(|l| {
            let c = split_csv(l);
            (c.len() >= 3).then(|| (c[0].clone(), c[1].clone(), c[2].clone()))
        })
        .collect())
}
