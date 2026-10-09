//! The service library of {{project-name}}: the business, and the composition root,
//! [`{{project-name | pascal_case}}`], which builds the services from the configuration. Every host runs the same
//! composition root: `{{project-name}}-bin` as a program of its own{% if jni %}, `{{project-name}}-jni`
//! inside an Android app{% endif %}. AGENTS.md says where code goes.

{% if http %}mod api;
{% endif %}mod heartbeat;

use rivium::{App, AppContext, Result, Service};{% if http %}
use rivium_http::{HttpServer, HttpSettings};{% endif %}
use serde::{Deserialize, Serialize};

/// The program, for every host.
pub struct {{project-name | pascal_case}};

impl App for {{project-name | pascal_case}} {
    const NAME: &'static str = "{{project-name}}";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    // The deployment's layout: configs/ next to the program, whose directory is the root.
    const CONFIG_FILE: &'static str = "configs/default.toml";
    type Config = Config;

    fn services(config: &Config, {% unless http %}_{% endunless %}ctx: &AppContext) -> Result<Vec<Box<dyn Service>>> {
{%- if http %}
        Ok(vec![
            Box::new(HttpServer::new(&config.http, api::router(), ctx)),
            heartbeat::service(&config.heartbeat),
        ])
{%- else %}
        Ok(vec![heartbeat::service(&config.heartbeat)])
{%- endif %}
    }
}

/// The configuration: a section per part of the service. Rivium's own sections, `[log]` and
/// `[lifecycle]`, are not part of it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
{%- if http %}
    /// `[http]`: where the API listens, and its limits.
    pub http: HttpSettings,
{%- endif %}
    /// `[heartbeat]`
    pub heartbeat: heartbeat::Settings,
}
