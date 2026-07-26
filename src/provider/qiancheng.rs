use async_trait::async_trait;
use reqwest::header::{ACCEPT, COOKIE, REFERER};
use serde_json::Value;

use super::{
    JobProvider, SearchRequest, cookie, first_text, overlay_list, overlay_text, parse_url,
    required_url, send_json, send_text, stable_id, text_list,
};
use crate::{BossError, Job, Platform};

/// 前程无忧 / 51job read-only search adapter.
pub struct QianchengProvider {
    client: reqwest::Client,
}

impl QianchengProvider {
    /// Creates the adapter.
    #[must_use]
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Builds the public search endpoint URL.
    pub fn search_url(request: &SearchRequest<'_>) -> Result<reqwest::Url, BossError> {
        let mut url = parse_url("https://we.51job.com/api/job/search-pc")?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("api_key", "51job")
                .append_pair("keyword", request.query)
                .append_pair("searchType", "2")
                .append_pair("sortType", "0")
                .append_pair("pageNum", &request.page.to_string())
                .append_pair("pageSize", &request.limit.to_string());
            if let Some(city) = request.city {
                pairs.append_pair(
                    "jobArea",
                    crate::city::provider_value(Platform::Qiancheng, city)?,
                );
            }
        }
        Ok(url)
    }

    fn parse(value: &Value) -> Result<Vec<Job>, BossError> {
        let items = value
            .pointer("/resultbody/job/items")
            .or_else(|| value.pointer("/resultbody/job/item"))
            .and_then(Value::as_array)
            .ok_or_else(|| BossError::Parse("missing resultbody.job.items".to_owned()))?;
        items.iter().map(Self::parse_job).collect()
    }

    fn parse_job(value: &Value) -> Result<Job, BossError> {
        let url = required_url(value.get("jobHref").and_then(Value::as_str), "jobHref")?;
        let remote_id = value
            .get("jobId")
            .and_then(Value::as_str)
            .unwrap_or(&url)
            .to_owned();
        let title = required_url(value.get("jobName").and_then(Value::as_str), "jobName")?;
        let mut job = Job::new(
            stable_id(Platform::Qiancheng, &remote_id, &url),
            Platform::Qiancheng,
            remote_id,
            title,
            url,
        );
        job.company = text(value, "fullCompanyName");
        job.city = text(value, "jobAreaString");
        job.district = first_text(value, &["/jobAreaLevelDetail", "/district"]);
        job.salary = text(value, "provideSalaryString");
        job.experience = first_text(value, &["/workYearString", "/workYear"]);
        job.education = first_text(value, &["/degreeString", "/degree"]);
        job.employment_type = first_text(value, &["/jobType", "/termStr"]);
        job.skills = text_list(value, &["/skillLabel", "/skills"]);
        job.welfare = text_list(value, &["/jobTags", "/welfare"]);
        Ok(job)
    }

    fn detail_url(job: &Job) -> Result<reqwest::Url, BossError> {
        let url = parse_url(&job.url).map_err(|_| {
            BossError::UnsafeProviderUrl("51job detail URL is malformed".to_owned())
        })?;
        let trusted_host = url.host_str().is_some_and(|host| {
            host == "51job.com"
                || host
                    .strip_suffix(".51job.com")
                    .is_some_and(|prefix| !prefix.is_empty())
        });
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || !trusted_host
        {
            return Err(BossError::UnsafeProviderUrl(
                "51job detail URL failed the HTTPS host trust policy".to_owned(),
            ));
        }
        Ok(url)
    }

    fn parse_detail_html(base: &Job, html: &str) -> Result<Job, BossError> {
        if html.trim().is_empty() {
            return Err(BossError::Parse("51job detail page was empty".to_owned()));
        }
        let tags = scan_html(html);
        if has_structural_challenge(&tags) {
            return Err(BossError::Parse(
                "51job detail page was blocked by risk control".to_owned(),
            ));
        }
        let mut job = base.clone();
        let mut posting = None;
        for script in tags.iter().filter(|tag| {
            tag.name.eq_ignore_ascii_case("script")
                && tag
                    .attribute("type")
                    .is_some_and(|value| value.eq_ignore_ascii_case("application/ld+json"))
        }) {
            let Some(raw) = script.content else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(raw) else {
                continue;
            };
            if let Some(candidate) = find_job_posting(&value).filter(|candidate| {
                !first_text(candidate, &["/title", "/name"])
                    .trim()
                    .is_empty()
            }) {
                posting = Some(candidate.clone());
                break;
            }
        }
        let posting = posting.ok_or_else(|| {
            BossError::Parse("51job detail page contained no valid JobPosting signal".to_owned())
        })?;
        overlay_json_ld(&mut job, &posting);
        overlay_meta_description(&mut job, &tags);
        if job.description.is_empty() {
            return Err(BossError::Parse(
                "51job detail page contained no JobPosting description".to_owned(),
            ));
        }
        Ok(job)
    }
}

#[async_trait]
impl JobProvider for QianchengProvider {
    fn platform(&self) -> Platform {
        Platform::Qiancheng
    }

    async fn search(&self, request: &SearchRequest<'_>) -> Result<Vec<Job>, BossError> {
        let mut builder = self
            .client
            .get(Self::search_url(request)?)
            .header(ACCEPT, "application/json")
            .header(REFERER, "https://we.51job.com/");
        if let Some(value) = cookie("BOSS_QIANCHENG_COOKIE") {
            builder = builder.header(COOKIE, value);
        }
        Self::parse(&send_json(builder).await?)
    }

    async fn detail(&self, job: &Job) -> Result<Job, BossError> {
        let mut builder = self
            .client
            .get(Self::detail_url(job)?)
            .header(ACCEPT, "text/html,application/xhtml+xml")
            .header(REFERER, "https://we.51job.com/");
        if let Some(value) = cookie("BOSS_QIANCHENG_COOKIE") {
            builder = builder.header(COOKIE, value);
        }
        Self::parse_detail_html(job, &send_text(builder).await?)
    }
}

fn find_job_posting(value: &Value) -> Option<&Value> {
    match value {
        Value::Array(items) => items.iter().find_map(find_job_posting),
        Value::Object(object) => {
            let is_job = object.get("@type").is_some_and(|kind| match kind {
                Value::String(kind) => kind == "JobPosting",
                Value::Array(kinds) => kinds.iter().any(|kind| kind == "JobPosting"),
                _ => false,
            });
            if is_job {
                Some(value)
            } else {
                object.get("@graph").and_then(find_job_posting)
            }
        }
        _ => None,
    }
}

fn overlay_json_ld(job: &mut Job, value: &Value) {
    overlay_text(&mut job.title, first_text(value, &["/title", "/name"]));
    overlay_text(
        &mut job.company,
        first_text(value, &["/hiringOrganization/name"]),
    );
    overlay_text(&mut job.description, first_text(value, &["/description"]));
    overlay_text(
        &mut job.employment_type,
        first_text(value, &["/employmentType"]),
    );
    overlay_text(
        &mut job.experience,
        first_text(value, &["/experienceRequirements"]),
    );
    overlay_text(
        &mut job.education,
        first_text(value, &["/educationRequirements"]),
    );
    overlay_text(
        &mut job.city,
        first_text(
            value,
            &[
                "/jobLocation/address/addressLocality",
                "/jobLocation/address/addressRegion",
            ],
        ),
    );
    overlay_text(
        &mut job.address,
        first_text(value, &["/jobLocation/address/streetAddress"]),
    );
    overlay_list(&mut job.skills, text_list(value, &["/skills"]));
}

fn overlay_meta_description(job: &mut Job, tags: &[HtmlTag<'_>]) {
    if !job.description.is_empty() {
        return;
    }
    if let Some(description) = tags.iter().find_map(|tag| {
        (tag.name.eq_ignore_ascii_case("meta")
            && tag
                .attribute("name")
                .is_some_and(|name| name.eq_ignore_ascii_case("description")))
        .then(|| tag.attribute("content"))
        .flatten()
    }) {
        overlay_text(&mut job.description, description.trim().to_owned());
    }
}

#[derive(Debug)]
struct HtmlAttribute<'a> {
    name: &'a str,
    value: Option<&'a str>,
}

#[derive(Debug)]
struct HtmlTag<'a> {
    name: &'a str,
    attributes: Vec<HtmlAttribute<'a>>,
    content: Option<&'a str>,
}

impl<'a> HtmlTag<'a> {
    fn attribute(&self, name: &str) -> Option<&'a str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
            .and_then(|attribute| attribute.value)
    }
}

fn scan_html(html: &str) -> Vec<HtmlTag<'_>> {
    let bytes = html.as_bytes();
    let mut cursor = 0;
    let mut tags = Vec::new();
    while let Some(relative) = bytes[cursor..].iter().position(|byte| *byte == b'<') {
        let start = cursor + relative;
        if bytes.get(start..start + 4) == Some(b"<!--") {
            cursor = bytes[start + 4..]
                .windows(3)
                .position(|window| window == b"-->")
                .map_or(bytes.len(), |relative| start + 4 + relative + 3);
            continue;
        }
        let mut name_start = start + 1;
        while bytes.get(name_start).is_some_and(u8::is_ascii_whitespace) {
            name_start += 1;
        }
        if bytes
            .get(name_start)
            .is_none_or(|byte| matches!(byte, b'/' | b'!' | b'?'))
        {
            cursor = name_start.saturating_add(1);
            continue;
        }
        let name_end = scan_name_end(bytes, name_start);
        if name_end == name_start {
            cursor = name_start.saturating_add(1);
            continue;
        }
        let name = &html[name_start..name_end];
        if !is_relevant_tag(name) {
            cursor = name_end;
            continue;
        }
        let Some(tag_end) = find_tag_end(bytes, name_end) else {
            cursor = name_end;
            continue;
        };
        let attributes = parse_attributes(html, name_end, tag_end);
        let content_start = tag_end + 1;
        let (content, next_cursor) = if tag_has_content(name) {
            find_closing_tag(html, content_start, name).map_or(
                (None, content_start),
                |(closing_start, closing_end)| {
                    (Some(&html[content_start..closing_start]), closing_end)
                },
            )
        } else {
            (None, content_start)
        };
        tags.push(HtmlTag {
            name,
            attributes,
            content,
        });
        cursor = next_cursor;
    }
    tags
}

fn scan_name_end(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start;
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'_'))
    {
        cursor += 1;
    }
    cursor
}

fn is_relevant_tag(name: &str) -> bool {
    ["script", "title", "meta", "textarea", "form"]
        .iter()
        .any(|expected| name.eq_ignore_ascii_case(expected))
}

fn tag_has_content(name: &str) -> bool {
    ["script", "title", "textarea"]
        .iter()
        .any(|expected| name.eq_ignore_ascii_case(expected))
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        match (*byte, quote) {
            (b'\'' | b'"', None) => quote = Some(*byte),
            (current, Some(expected)) if current == expected => quote = None,
            (b'>', None) => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn parse_attributes(html: &str, start: usize, end: usize) -> Vec<HtmlAttribute<'_>> {
    let bytes = html.as_bytes();
    let mut cursor = start;
    let mut attributes = Vec::new();
    while cursor < end {
        while cursor < end && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b'/') {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < end
            && !bytes[cursor].is_ascii_whitespace()
            && !matches!(bytes[cursor], b'=' | b'/' | b'>')
        {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = &html[name_start..cursor];
        while cursor < end && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let value = if cursor < end && bytes[cursor] == b'=' {
            cursor += 1;
            while cursor < end && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            Some(parse_attribute_value(html, &mut cursor, end))
        } else {
            None
        };
        attributes.push(HtmlAttribute { name, value });
    }
    attributes
}

fn parse_attribute_value<'a>(html: &'a str, cursor: &mut usize, end: usize) -> &'a str {
    let bytes = html.as_bytes();
    if *cursor >= end {
        return "";
    }
    if matches!(bytes[*cursor], b'\'' | b'"') {
        let quote = bytes[*cursor];
        *cursor += 1;
        let value_start = *cursor;
        while *cursor < end && bytes[*cursor] != quote {
            *cursor += 1;
        }
        let value = &html[value_start..*cursor];
        if *cursor < end {
            *cursor += 1;
        }
        value
    } else {
        let value_start = *cursor;
        while *cursor < end && !bytes[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        &html[value_start..*cursor]
    }
}

fn find_closing_tag(html: &str, start: usize, name: &str) -> Option<(usize, usize)> {
    let bytes = html.as_bytes();
    let mut cursor = start;
    while let Some(relative) = bytes[cursor..].iter().position(|byte| *byte == b'<') {
        let opening = cursor + relative;
        let mut name_start = opening + 1;
        if bytes.get(name_start) != Some(&b'/') {
            cursor = name_start;
            continue;
        }
        name_start += 1;
        while bytes.get(name_start).is_some_and(u8::is_ascii_whitespace) {
            name_start += 1;
        }
        let name_end = scan_name_end(bytes, name_start);
        if html[name_start..name_end].eq_ignore_ascii_case(name)
            && bytes
                .get(name_end)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
            && let Some(tag_end) = find_tag_end(bytes, name_end)
        {
            return Some((opening, tag_end + 1));
        }
        cursor = name_end.max(name_start + 1);
    }
    None
}

fn has_structural_challenge(tags: &[HtmlTag<'_>]) -> bool {
    tags.iter().any(|tag| {
        (tag.name.eq_ignore_ascii_case("textarea")
            && tag
                .attribute("id")
                .is_some_and(|id| id.eq_ignore_ascii_case("renderData"))
            && tag
                .content
                .is_some_and(|content| contains_ascii_case_insensitive(content, "_waf_")))
            || (tag.name.eq_ignore_ascii_case("meta")
                && tag
                    .attribute("name")
                    .is_some_and(|name| starts_with_ascii_case_insensitive(name, "aliyun_waf_")))
            || (tag.name.eq_ignore_ascii_case("title")
                && tag.content.is_some_and(is_challenge_title))
            || (tag.name.eq_ignore_ascii_case("form")
                && ["id", "name", "class", "action"]
                    .iter()
                    .any(|name| tag.attribute(name).is_some_and(is_challenge_form_marker)))
    })
}

fn is_challenge_title(title: &str) -> bool {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    [
        "captcha",
        "challenge",
        "security verification",
        "security-verification",
        "安全验证",
        "滑块验证",
        "访问验证",
    ]
    .iter()
    .any(|marker| normalized.eq_ignore_ascii_case(marker))
}

fn is_challenge_form_marker(value: &str) -> bool {
    [
        "captcha",
        "challenge",
        "security-verification",
        "security_verification",
        "aliyun_waf",
    ]
    .iter()
    .any(|marker| contains_ascii_case_insensitive(value, marker))
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn contains_ascii_case_insensitive(value: &str, needle: &str) -> bool {
    value
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_response_are_normalized() {
        let request = SearchRequest {
            query: "rust",
            city: Some("深圳"),
            page: 1,
            limit: 10,
        };
        let url = QianchengProvider::search_url(&request).expect("url");
        assert!(url.as_str().contains("keyword=rust"));
        assert!(url.as_str().contains("jobArea=040000"));
        let value =
            serde_json::from_str(include_str!("fixtures/qiancheng_search.json")).expect("fixture");
        let jobs = QianchengProvider::parse(&value).expect("parse");
        assert_eq!(
            (
                &jobs[0].remote_id,
                &jobs[0].experience,
                jobs[0].welfare.len()
            ),
            (&"sanitized-51-001".to_owned(), &"3-5年".to_owned(), 2)
        );
    }

    #[test]
    fn graph_json_ld_fixture_overlays_detail() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let detail = QianchengProvider::parse_detail_html(
            &base,
            include_str!("fixtures/qiancheng_detail.html"),
        )
        .expect("detail");

        assert_eq!(
            (detail.title, detail.description, detail.address),
            (
                "Rust Systems Engineer".to_owned(),
                "Build reliable Linux platform services.".to_owned(),
                "科技园示例路".to_owned(),
            )
        );
    }

    #[test]
    fn object_and_array_json_ld_shapes_are_accepted() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let object = r#"<script type="application/ld+json">{"@type":"JobPosting","name":"Object title","description":"Object description"}</script>"#;
        let array = r#"<script type="application/ld+json">[{"@type":"Thing"},{"@type":"JobPosting","title":"Array title","description":"Array description"}]</script>"#;

        assert_eq!(
            (
                QianchengProvider::parse_detail_html(&base, object)
                    .expect("object")
                    .title,
                QianchengProvider::parse_detail_html(&base, array)
                    .expect("array")
                    .title,
            ),
            ("Object title".to_owned(), "Array title".to_owned())
        );
    }

    #[test]
    fn malformed_or_meta_only_html_is_rejected() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let malformed = r#"<script type="application/ld+json">{"@type":"JobPosting",</script>"#;
        let meta_only = r#"<meta name="description" content="looks like a job">"#;

        assert!(
            QianchengProvider::parse_detail_html(&base, malformed).is_err()
                && QianchengProvider::parse_detail_html(&base, meta_only).is_err()
        );
    }

    #[test]
    fn realistic_waf_and_captcha_pages_are_rejected() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );

        assert!(
            QianchengProvider::parse_detail_html(
                &base,
                include_str!("fixtures/qiancheng_blocked_aliyun.html")
            )
            .is_err()
                && QianchengProvider::parse_detail_html(
                    &base,
                    include_str!("fixtures/qiancheng_blocked_captcha.html")
                )
                .is_err()
        );
    }

    #[test]
    fn ordinary_job_security_terms_do_not_trigger_risk_control() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let detail = QianchengProvider::parse_detail_html(
            &base,
            include_str!("fixtures/qiancheng_detail_security_terms.html"),
        )
        .expect("detail");

        assert_eq!(
            (detail.title, detail.description),
            (
                "Aliyun WAF Security Challenge Engineer".to_owned(),
                "Build Aliyun WAF security and challenge-response services.".to_owned(),
            )
        );
    }

    #[test]
    fn scanner_handles_case_insensitive_tags_and_attribute_quoting() {
        let html = r#"<ScRiPt DATA-one='a>b' data-two="c>d" TYPE=application/ld+json>{"ok":true}</sCrIpT>"#;
        let tags = scan_html(html);
        let script = tags.first().expect("script");

        assert_eq!(
            (
                script.name,
                script.attribute("DATA-ONE"),
                script.attribute("data-two"),
                script.attribute("type"),
                script.content,
            ),
            (
                "ScRiPt",
                Some("a>b"),
                Some("c>d"),
                Some("application/ld+json"),
                Some(r#"{"ok":true}"#),
            )
        );
    }

    #[test]
    fn malformed_json_script_does_not_hide_later_valid_script() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let html = r#"
            <div broken='irrelevant>
            <script type='application/ld+json'>{"@type":"JobPosting",</script>
            <SCRIPT TYPE="application/ld+json">{"@type":"JobPosting","title":"Valid second","description":"Found"}</SCRIPT>
        "#;
        let detail = QianchengProvider::parse_detail_html(&base, html).expect("detail");

        assert_eq!(
            (detail.title, detail.description),
            ("Valid second".to_owned(), "Found".to_owned())
        );
    }

    #[test]
    fn commented_job_posting_alone_is_not_a_signal() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let html = r#"<!-- <script type="application/ld+json">{"@type":"JobPosting","title":"Fake","description":"Fake"}</script> -->"#;

        assert!(QianchengProvider::parse_detail_html(&base, html).is_err());
    }

    #[test]
    fn commented_challenge_title_does_not_block_live_job_posting() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let html = r#"
            <!-- <title>captcha</title> -->
            <script type="application/ld+json">{"@type":"JobPosting","title":"Live","description":"Real"}</script>
        "#;
        let detail = QianchengProvider::parse_detail_html(&base, html).expect("detail");

        assert_eq!(detail.title, "Live");
    }

    #[test]
    fn commented_fake_job_does_not_replace_live_real_job() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let html = r#"
            <!-- <script type="application/ld+json">{"@type":"JobPosting","title":"Fake","description":"Fake"}</script> -->
            <meta name="description" content="Live fallback">
            <!-- an ordinary comment between legitimate tags -->
            <script type="application/ld+json">{"@type":"JobPosting","title":"Real","description":"Real description"}</script>
        "#;
        let detail = QianchengProvider::parse_detail_html(&base, html).expect("detail");

        assert_eq!(
            (detail.title, detail.description),
            ("Real".to_owned(), "Real description".to_owned())
        );
    }

    #[test]
    fn unclosed_comment_hides_all_remaining_job_signals() {
        let base = Job::new(
            "id",
            Platform::Qiancheng,
            "51",
            "Old",
            "https://we.51job.com/job/51",
        );
        let html = r#"<!-- never closed <script type="application/ld+json">{"@type":"JobPosting","title":"Fake","description":"Fake"}</script>"#;

        assert!(QianchengProvider::parse_detail_html(&base, html).is_err());
    }

    #[test]
    fn detail_url_accepts_only_trusted_https_51job_hosts() {
        for accepted in [
            "https://51job.com/job/1",
            "https://we.51job.com/job/1",
            "https://sub.we.51job.com:443/job/1",
        ] {
            let job = Job::new("id", Platform::Qiancheng, "1", "Rust", accepted);
            assert!(QianchengProvider::detail_url(&job).is_ok(), "{accepted}");
        }
        for rejected in [
            "http://we.51job.com/job/1",
            "https://user@we.51job.com/job/1",
            "https://user:secret@we.51job.com/job/1",
            "https://we.51job.com:444/job/1",
            "https://evil51job.com/job/1",
            "https://51job.com.evil.test/job/1",
            "https://127.0.0.1/job/1",
            "https://localhost/job/1",
            "file:///etc/passwd",
        ] {
            let job = Job::new("id", Platform::Qiancheng, "1", "Rust", rejected);
            let error = QianchengProvider::detail_url(&job).expect_err(rejected);
            assert_eq!(error.code(), "unsafe_provider_url", "{rejected}");
        }
    }
}
