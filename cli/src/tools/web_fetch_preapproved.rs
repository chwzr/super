/// Preapproved hosts for WebFetch. Ported from Claude Code's
/// `WebFetchTool/preapproved.ts`. Entries with a '/' have a path prefix
/// (e.g. "github.com/anthropics"); bare hostnames match any path.
static PREAPPROVED_HOSTS: &[&str] = &[
    // Anthropic
    "platform.claude.com",
    "code.claude.com",
    "modelcontextprotocol.io",
    "github.com/anthropics",
    "agentskills.io",
    // Top Programming Languages
    "docs.python.org",
    "en.cppreference.com",
    "docs.oracle.com",
    "learn.microsoft.com",
    "developer.mozilla.org",
    "go.dev",
    "pkg.go.dev",
    "www.php.net",
    "docs.swift.org",
    "kotlinlang.org",
    "ruby-doc.org",
    "doc.rust-lang.org",
    "www.typescriptlang.org",
    // Web & JavaScript
    "react.dev",
    "angular.io",
    "vuejs.org",
    "nextjs.org",
    "expressjs.com",
    "nodejs.org",
    "bun.sh",
    "jquery.com",
    "getbootstrap.com",
    "tailwindcss.com",
    "d3js.org",
    "threejs.org",
    "redux.js.org",
    "webpack.js.org",
    "jestjs.io",
    "reactrouter.com",
    // Python
    "docs.djangoproject.com",
    "flask.palletsprojects.com",
    "fastapi.tiangolo.com",
    "pandas.pydata.org",
    "numpy.org",
    "www.tensorflow.org",
    "pytorch.org",
    "scikit-learn.org",
    "matplotlib.org",
    "requests.readthedocs.io",
    "jupyter.org",
    // PHP
    "laravel.com",
    "symfony.com",
    "wordpress.org",
    // Java
    "docs.spring.io",
    "hibernate.org",
    "tomcat.apache.org",
    "gradle.org",
    "maven.apache.org",
    // .NET
    "asp.net",
    "dotnet.microsoft.com",
    "nuget.org",
    "blazor.net",
    // Mobile
    "reactnative.dev",
    "docs.flutter.dev",
    "developer.apple.com",
    "developer.android.com",
    // Data Science
    "keras.io",
    "spark.apache.org",
    "huggingface.co",
    "www.kaggle.com",
    // Databases
    "www.mongodb.com",
    "redis.io",
    "www.postgresql.org",
    "dev.mysql.com",
    "www.sqlite.org",
    "graphql.org",
    "prisma.io",
    // Cloud & DevOps
    "docs.aws.amazon.com",
    "cloud.google.com",
    "kubernetes.io",
    "www.docker.com",
    "www.terraform.io",
    "www.ansible.com",
    "vercel.com/docs",
    "docs.netlify.com",
    "devcenter.heroku.com",
    // Testing
    "cypress.io",
    "selenium.dev",
    // Game Development
    "docs.unity.com",
    "docs.unrealengine.com",
    // Other
    "git-scm.com",
    "nginx.org",
    "httpd.apache.org",
];

/// Returns true if the given URL is a preapproved domain.
///
/// For bare hostname entries, any path on that host matches.
/// For path-prefixed entries (e.g. "github.com/anthropics"), only paths
/// starting with the prefix are allowed. Path-segment boundaries are
/// enforced: "/anthropics" must not match "/anthropics-evil/...".
pub fn is_preapproved_url(url: &str) -> bool {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    let hostname = parsed.host_str().unwrap_or("");
    let path = parsed.path();

    for entry in PREAPPROVED_HOSTS {
        if let Some(slash_pos) = entry.find('/') {
            let (entry_host, entry_path) = entry.split_at(slash_pos);
            if entry_host == hostname {
                // Path-prefixed entry: enforce segment boundary
                if path == entry_path || path.starts_with(&format!("{}/", entry_path)) {
                    return true;
                }
            }
        } else if *entry == hostname {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bare_hostname() {
        assert!(is_preapproved_url("https://doc.rust-lang.org/std/vec"));
        assert!(is_preapproved_url(
            "https://docs.python.org/3/library/os.html"
        ));
        assert!(is_preapproved_url(
            "https://react.dev/reference/react/useState"
        ));
    }

    #[test]
    fn test_path_prefixed() {
        assert!(is_preapproved_url("https://github.com/anthropics"));
        assert!(is_preapproved_url(
            "https://github.com/anthropics/superpowers"
        ));
        // Path segment boundary: must not match /anthropics-evil
        assert!(!is_preapproved_url("https://github.com/anthropics-evil"));
    }

    #[test]
    fn test_not_preapproved() {
        assert!(!is_preapproved_url("https://evil.example.com"));
        assert!(!is_preapproved_url("https://random-blog.com/post"));
    }

    #[test]
    fn test_invalid_url() {
        assert!(!is_preapproved_url("not-a-url"));
    }

    #[test]
    fn test_vercel_docs_path_prefix() {
        assert!(is_preapproved_url("https://vercel.com/docs"));
        assert!(is_preapproved_url("https://vercel.com/docs/deployments"));
        // Other paths on vercel.com should NOT match
        assert!(!is_preapproved_url("https://vercel.com"));
        assert!(!is_preapproved_url("https://vercel.com/pricing"));
    }
}
