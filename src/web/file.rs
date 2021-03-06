//! Database based file handler

use crate::storage::{Blob, Storage};
use crate::{error::Result, Config, Metrics};
use iron::{status, Handler, IronResult, Request, Response};

#[derive(Debug)]
pub(crate) struct File(pub(crate) Blob);

impl File {
    /// Gets file from database
    pub fn from_path(storage: &Storage, path: &str, config: &Config) -> Result<File> {
        let max_size = if path.ends_with(".html") {
            config.max_file_size_html
        } else {
            config.max_file_size
        };

        Ok(File(storage.get(path, max_size)?))
    }

    /// Consumes File and creates a iron response
    pub fn serve(self) -> Response {
        use iron::headers::{CacheControl, CacheDirective, ContentType, HttpDate, LastModified};

        let mut response = Response::with((status::Ok, self.0.content));
        let cache = vec![
            CacheDirective::Public,
            CacheDirective::MaxAge(super::STATIC_FILE_CACHE_DURATION as u32),
        ];
        response
            .headers
            .set(ContentType(self.0.mime.parse().unwrap()));
        response.headers.set(CacheControl(cache));
        // FIXME: This is so horrible
        response.headers.set(LastModified(HttpDate(
            time::strptime(
                &self.0.date_updated.format("%a, %d %b %Y %T %Z").to_string(),
                "%a, %d %b %Y %T %Z",
            )
            .unwrap(),
        )));
        response
    }

    /// Checks if mime type of file is "application/x-empty"
    pub fn is_empty(&self) -> bool {
        self.0.mime == "application/x-empty"
    }
}

/// Database based file handler for iron
///
/// This is similar to staticfile crate, but its using getting files from database.
pub struct DatabaseFileHandler;

impl Handler for DatabaseFileHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        let path = req.url.path().join("/");
        let storage = extension!(req, Storage);
        let config = extension!(req, Config);
        if let Ok(file) = File::from_path(&storage, &path, &config) {
            let metrics = extension!(req, Metrics);

            // Because all requests that don't hit another handler go through here, we will get all
            // requests successful or not recorded by the RequestRecorder, so we inject an extra
            // "database success" route to keep track of how often we succeed vs 404
            metrics
                .routes_visited
                .with_label_values(&["database success"])
                .inc();
            Ok(file.serve())
        } else {
            Err(super::error::Nope::CrateNotFound.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use chrono::Utc;

    #[test]
    fn file_roundtrip() {
        wrapper(|env| {
            let now = Utc::now();

            env.fake_release().create()?;

            let mut file = File::from_path(
                &env.storage(),
                "rustdoc/fake-package/1.0.0/fake-package/index.html",
                &env.config(),
            )
            .unwrap();
            file.0.date_updated = now;

            let resp = file.serve();
            assert_eq!(
                resp.headers.get_raw("Last-Modified").unwrap(),
                [now.format("%a, %d %b %Y %T GMT").to_string().into_bytes()].as_ref(),
            );

            Ok(())
        });
    }

    #[test]
    fn test_max_size() {
        const MAX_SIZE: usize = 1024;
        const MAX_HTML_SIZE: usize = 128;

        wrapper(|env| {
            env.override_config(|config| {
                config.max_file_size = MAX_SIZE;
                config.max_file_size_html = MAX_HTML_SIZE;
            });

            env.fake_release()
                .name("dummy")
                .version("0.1.0")
                .rustdoc_file_with("small.html", &[b'A'; MAX_HTML_SIZE / 2] as &[u8])
                .rustdoc_file_with("exact.html", &[b'A'; MAX_HTML_SIZE] as &[u8])
                .rustdoc_file_with("big.html", &[b'A'; MAX_HTML_SIZE * 2] as &[u8])
                .rustdoc_file_with("small.js", &[b'A'; MAX_SIZE / 2] as &[u8])
                .rustdoc_file_with("exact.js", &[b'A'; MAX_SIZE] as &[u8])
                .rustdoc_file_with("big.js", &[b'A'; MAX_SIZE * 2] as &[u8])
                .create()?;

            let file = |path| {
                File::from_path(
                    &env.storage(),
                    &format!("rustdoc/dummy/0.1.0/{}", path),
                    &env.config(),
                )
            };
            let assert_len = |len, path| {
                assert_eq!(len, file(path).unwrap().0.content.len());
            };
            let assert_too_big = |path| {
                file(path)
                    .unwrap_err()
                    .downcast_ref::<std::io::Error>()
                    .and_then(|io| io.get_ref())
                    .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                    .is_some()
            };

            assert_len(MAX_HTML_SIZE / 2, "small.html");
            assert_len(MAX_HTML_SIZE, "exact.html");
            assert_len(MAX_SIZE / 2, "small.js");
            assert_len(MAX_SIZE, "exact.js");

            assert_too_big("big.html");
            assert_too_big("big.js");

            Ok(())
        })
    }
}
