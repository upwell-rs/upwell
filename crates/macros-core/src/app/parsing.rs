use std::collections::HashSet;

use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, LitBool, Token, Type, braced, bracketed, parenthesized};

use super::model::{
    AppPhases, CliDeclarations, ConfigEntry, ConfigSettings, Declared, DirSettings, ManagerSetting,
    ManagerSource, ManagerValue, PhaseArgument, PhaseInput, PluginDirective,
};
use super::{AppAssembly, NamedApp, command};

syn::custom_keyword!(app);
syn::custom_keyword!(replace);
syn::custom_keyword!(suppress);

fn parse_config_settings(input: ParseStream) -> syn::Result<ConfigSettings> {
    let mut source = None;
    let mut profiles = None;
    let mut sighup = None;
    let mut watch = None;
    let mut debounce = None;
    let mut keys = HashSet::new();

    while !input.is_empty() {
        let key = input.call(Ident::parse_any)?;
        let name = key.to_string();

        duplicate_manager_setting(&mut keys, &key, &name, "config")?;

        input.parse::<Token![:]>()?;

        match name.as_str() {
            "source" => source = Some(manager_setting(&key, input.parse()?)),
            "profiles" => profiles = Some(manager_setting(&key, input.parse()?)),
            "sighup" => {
                sighup = Some(manager_setting(&key, input.parse::<LitBool>()?.value));
            }
            "watch" => {
                watch = Some(manager_setting(&key, input.parse::<LitBool>()?.value));
            }
            "debounce" => debounce = Some(manager_setting(&key, input.parse()?)),
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!(
                        "unknown `config` setting `{other}`; expected `source`, `profiles`, \
                             `sighup`, `watch`, or `debounce`"
                    ),
                ));
            }
        }

        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }

    if source.is_some()
        && let Some(profiles) = &profiles
    {
        return Err(syn::Error::new(
            profiles.key_span,
            "`config` settings cannot combine `source` with `profiles`",
        ));
    }

    Ok(ConfigSettings {
        source,
        profiles,
        sighup,
        watch,
        debounce,
    })
}

fn parse_dir_settings(input: ParseStream) -> syn::Result<DirSettings> {
    let mut app = None;
    let mut root = None;
    let mut keys = HashSet::new();

    while !input.is_empty() {
        let key = input.call(Ident::parse_any)?;
        let name = key.to_string();

        duplicate_manager_setting(&mut keys, &key, &name, "directories")?;

        input.parse::<Token![:]>()?;

        match name.as_str() {
            "app" => app = Some(manager_setting(&key, input.parse()?)),
            "root" => root = Some(manager_setting(&key, input.parse()?)),
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!("unknown `directories` setting `{other}`; expected `app` or `root`"),
                ));
            }
        }

        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }

    if app.is_some()
        && let Some(root) = &root
    {
        return Err(syn::Error::new(
            root.key_span,
            "`directories` settings cannot combine `app` with `root`",
        ));
    }

    Ok(DirSettings { app, root })
}

impl Parse for ConfigEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty = input.parse()?;

        input.parse::<Token![=>]>()?;

        let path = input.parse()?;

        Ok(ConfigEntry { ty, path })
    }
}

impl Parse for NamedApp {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attributes = input.call(syn::Attribute::parse_outer)?;
        let visibility = input.parse()?;

        input.parse::<app>()?;

        let ident = input.parse()?;
        let content;

        braced!(content in input);

        let assembly = AppAssembly::parse_with(&content)?;

        if !input.is_empty() {
            return Err(input.error("unexpected tokens after named app definition"));
        }

        for attribute in &attributes {
            if !attribute.path().is_ident("doc") {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "only documentation attributes are supported on generated applications",
                ));
            }
        }

        Ok(Self {
            attributes,
            visibility,
            ident,
            assembly,
        })
    }
}

impl AppAssembly {
    fn parse_with(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut protocol = None;
        let mut services = Vec::new();
        let mut components = Vec::new();
        let mut configs = Vec::new();
        let mut config_manager = None;
        let mut directories_manager = None;
        let mut middleware = Vec::new();
        let mut guards = Vec::new();
        let mut error_handler = None;
        let mut deny_unplanned = false;
        let mut plugins = Vec::new();
        let mut upwell = None;
        let mut phases = AppPhases::default();
        let mut cli_policy = super::policy::CliPolicy::default();
        let mut cli = CliDeclarations::default();
        let mut keys = HashSet::new();

        while !input.is_empty() {
            let key = input.call(Ident::parse_any)?;
            let key_name = key.to_string();

            if !keys.insert(key_name.clone()) {
                return Err(syn::Error::new(
                    key.span(),
                    format!("duplicate app key `{key_name}`"),
                ));
            }

            if is_lifecycle_phase(&key_name) {
                let phase = parse_phase(input, &key)?;

                set_phase(&mut phases, &key, phase);

                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }

                continue;
            }

            input.parse::<Token![:]>()?;

            match key_name.as_str() {
                "name" => name = Some(declared(key, input.parse()?)),
                "protocol" => protocol = Some(declared(key, input.parse()?)),
                "services" => services = bracketed_list::<Type>(input)?,
                "components" => components = bracketed_list(input)?,
                "configs" => configs = bracketed_list(input)?,
                "managers" => parse_managers(input, &mut config_manager, &mut directories_manager)?,
                "middleware" => middleware = bracketed_list(input)?,
                "guards" => guards = bracketed_list(input)?,
                "error_handler" => error_handler = Some(input.parse()?),
                "deny_unplanned" => deny_unplanned = input.parse::<LitBool>()?.value,
                "plugins" => plugins = parse_plugins(input)?,
                "upwell" => upwell = Some(input.parse()?),
                "cli" => cli_policy = super::policy::parse(input)?,
                "args" => cli.args = command::parse_args(input)?,
                "commands" => cli.commands = command::parse_commands(input)?,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown `app!` key `{other}`, expected `name`, `protocol`, \
                             `services`, `components`, `configs`, `managers`, `middleware`, \
                             `guards`, `error_handler`, `deny_unplanned`, `plugins`, `cli`, \
                             `args`, `commands`, `upwell`, `setup`, `configure`, `before_build`, \
                             `after_build`, or `serve`"
                        ),
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let name = name.ok_or_else(|| input.error("`app!` requires a `name`"))?;
        let protocol = protocol.ok_or_else(|| {
            input.error("`app!` requires a `protocol: <ProtocolDefinition>` (e.g. `Rpc`)")
        })?;

        validate_managers(&config_manager, &directories_manager)?;
        command::validate_literal_collisions(&cli.commands, &cli_policy, phases.serve.is_some())?;

        Ok(Self {
            name,
            protocol,
            services,
            components,
            configs,
            config_manager,
            directories_manager,
            middleware,
            guards,
            error_handler,
            deny_unplanned,
            plugins,
            upwell,
            phases,
            cli_policy,
            cli,
        })
    }
}

fn parse_plugins(input: ParseStream) -> syn::Result<Vec<PluginDirective>> {
    let content;

    bracketed!(content in input);

    let mut directives = Vec::new();

    while !content.is_empty() {
        let directive = if content.peek(replace) {
            content.parse::<replace>()?;

            let slot = content.parse()?;

            content.parse::<Token![=>]>()?;

            let plugin = content.parse()?;

            PluginDirective::Replace { slot, plugin }
        } else if content.peek(suppress) {
            content.parse::<suppress>()?;

            PluginDirective::Suppress(content.parse()?)
        } else {
            PluginDirective::Install(content.parse()?)
        };

        directives.push(directive);

        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        } else if !content.is_empty() {
            return Err(content.error("expected `,` between plugin directives"));
        }
    }

    Ok(directives)
}

fn is_lifecycle_phase(key: &str) -> bool {
    matches!(
        key,
        "setup" | "configure" | "before_build" | "after_build" | "serve"
    )
}

fn set_phase(phases: &mut AppPhases, key: &Ident, phase: PhaseInput) {
    let phase = Declared {
        key: key.clone(),
        value: phase,
    };

    match key.to_string().as_str() {
        "setup" => phases.setup = Some(phase),
        "configure" => phases.configure = Some(phase),
        "before_build" => phases.before_build = Some(phase),
        "after_build" => phases.after_build = Some(phase),
        "serve" => phases.serve = Some(phase),
        _ => unreachable!(),
    }
}

fn declared<T>(key: Ident, value: T) -> Declared<T> {
    Declared { key, value }
}

fn parse_phase(input: ParseStream, key: &Ident) -> syn::Result<PhaseInput> {
    if input.peek(Token![=]) {
        input.parse::<Token![=]>()?;

        return Ok(PhaseInput::Path(input.parse()?));
    }

    if input.peek(syn::token::Paren) {
        let arguments;

        parenthesized!(arguments in input);

        let arguments = Punctuated::<PhaseArgument, Token![,]>::parse_terminated(&arguments)?
            .into_iter()
            .collect::<Vec<_>>();
        let body = input.parse()?;
        let expected_arguments = if key == "setup" { 1 } else { 2 };

        if arguments.len() < expected_arguments
            || (key != "serve" && arguments.len() != expected_arguments)
        {
            return Err(syn::Error::new(
                key.span(),
                format!(
                    "`{key}` expects {expected_arguments} argument{}",
                    if expected_arguments == 1 { "" } else { "s" }
                ),
            ));
        }

        for argument in &arguments[..expected_arguments] {
            if argument.ty.is_some() {
                return Err(syn::Error::new(
                    argument.ident.span(),
                    "lifecycle context and app parameters cannot declare injected types",
                ));
            }
        }

        for argument in &arguments[expected_arguments..] {
            if argument.ty.is_none() {
                return Err(syn::Error::new(
                    argument.ident.span(),
                    "additional serve parameters require an injectable type",
                ));
            }
        }

        return Ok(PhaseInput::Inline { arguments, body });
    }

    if input.peek(Token![:]) {
        return Err(syn::Error::new(
            key.span(),
            "declarative lifecycle settings are reserved for the generated CLI bootstrap; use `phase = async_function` or `phase(args...) { ... }`",
        ));
    }

    Err(syn::Error::new(
        key.span(),
        "expected `= async_function` or `(arguments...) { ... }` after lifecycle phase",
    ))
}

impl Parse for PhaseArgument {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident = input.call(Ident::parse_any)?;
        let ty = if input.peek(Token![:]) {
            input.parse::<Token![:]>()?;

            Some(input.parse()?)
        } else {
            None
        };

        Ok(Self { ident, ty })
    }
}

fn parse_managers(
    input: ParseStream,
    config: &mut Option<ManagerSource<ConfigSettings>>,
    directories: &mut Option<ManagerSource<DirSettings>>,
) -> syn::Result<()> {
    let content;

    braced!(content in input);

    while !content.is_empty() {
        let key: Ident = content.parse()?;

        content.parse::<Token![:]>()?;

        match key.to_string().as_str() {
            "config" => {
                if config.is_some() {
                    return Err(syn::Error::new(key.span(), "duplicate `config` manager"));
                }

                *config = Some(parse_manager_source(&content, &key, parse_config_settings)?);
            }
            "directories" => {
                if directories.is_some() {
                    return Err(syn::Error::new(
                        key.span(),
                        "duplicate `directories` manager",
                    ));
                }

                *directories = Some(parse_manager_source(&content, &key, parse_dir_settings)?);
            }
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!("unknown manager `{other}`, expected `config` or `directories`"),
                ));
            }
        }

        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }

    Ok(())
}

fn parse_manager_source<S>(
    input: ParseStream,
    key: &Ident,
    parse_settings: impl FnOnce(ParseStream) -> syn::Result<S>,
) -> syn::Result<ManagerSource<S>> {
    let key_span = key.span();

    if input.peek(syn::token::Brace) {
        let content;

        let brace = braced!(content in input);
        let settings = parse_settings(&content)?;

        return Ok(ManagerSource {
            key_span,
            value: ManagerValue::Configure {
                block_span: brace.span.join(),
                settings,
            },
        });
    }

    Ok(ManagerSource {
        key_span,
        value: ManagerValue::Instance(input.parse()?),
    })
}

fn manager_setting<T>(key: &Ident, value: T) -> ManagerSetting<T> {
    ManagerSetting {
        key_span: key.span(),
        value,
    }
}

fn duplicate_manager_setting(
    keys: &mut HashSet<String>,
    key: &Ident,
    name: &str,
    manager: &str,
) -> syn::Result<()> {
    if !keys.insert(name.to_owned()) {
        return Err(syn::Error::new(
            key.span(),
            format!("duplicate `{manager}` setting `{name}`"),
        ));
    }

    Ok(())
}

fn validate_managers(
    config: &Option<ManagerSource<ConfigSettings>>,
    directories: &Option<ManagerSource<DirSettings>>,
) -> syn::Result<()> {
    if let Some(ManagerSource {
        value: ManagerValue::Configure {
            block_span,
            settings,
        },
        ..
    }) = directories
        && settings.app.is_none()
        && settings.root.is_none()
    {
        return Err(syn::Error::new(
            *block_span,
            "a `directories` config block needs `app` or `root`",
        ));
    }

    if let Some(ManagerSource {
        key_span,
        value: ManagerValue::Configure { settings, .. },
    }) = config
        && settings.source.is_none()
        && directories.is_none()
    {
        return Err(syn::Error::new(
            *key_span,
            "a `config` block without `source` requires a `directories` manager to load from",
        ));
    }

    Ok(())
}

fn bracketed_list<T: Parse>(input: ParseStream) -> syn::Result<Vec<T>> {
    let content;

    bracketed!(content in input);

    let list = Punctuated::<T, Token![,]>::parse_terminated(&content)?;

    Ok(list.into_iter().collect())
}
