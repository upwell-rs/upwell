//! The jobs extension: `Jobs`, the `ParseMethod` extension that makes `#[jobs]` =
//! `MethodArgs<Jobs>` (`#[methods]` + job registration).
//!
//! `Jobs` claims each `#[job]` method — building its erased call (which resolves the `&self`
//! receiver and each injected parameter from the root scope) and its `JobDescriptor` — and on
//! emission appends them to the global `JOBS` slice. The base
//! [`MethodArgs`](upwell_macros_core::methods::MethodArgs) handles `#[init]`/`#[hook]`, so a
//! `#[jobs]` block supports those too. Because jobs contribute no client surface,
//! [`ParseMethod::parse_method`] returns `Ok(None)`.
//!
//! Unlike a handler, a job has no request context, so its parameters are plain `FromContainer`
//! types (`Dep<T>`, `Arc<T>`, `Cfg<T>`, …) resolved on each run — the shapes an `#[init]`
//! constructor takes — rather than the `Inject<_>` wrapper handlers use.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::parse::ParseStream;
use syn::{FnArg, Ident, ImplItemFn, ItemImpl, LitStr, Meta, Token, Type};
use upwell_macros_core::attr;
use upwell_macros_core::extend::{ParseItem, ParseKeyed, ParseMethod};
use upwell_macros_core::methods::self_ty_ident;
use upwell_macros_core::paths::Paths;

/// The jobs extension. Accumulates the impl's `#[job]` call/descriptor blocks and the captured
/// impl context, then emits them inside one `const` block.
#[derive(Default)]
pub struct Jobs {
    context: Option<JobContext>,
    blocks: Vec<TokenStream>,
}

/// The impl context `Jobs` needs to emit (captured in the item pass).
struct JobContext {
    self_ty: Type,
    self_name: LitStr,
    paths: Paths,
}

/// Which schedule flavour a `#[job]` uses, mapped to the `ScheduleKind` variant emitted.
#[derive(Clone, Copy)]
enum JobKind {
    Interval,
    Cron,
}

impl JobKind {
    fn variant(self) -> &'static str {
        match self {
            JobKind::Interval => "Interval",
            JobKind::Cron => "Cron",
        }
    }
}

impl ParseKeyed for Jobs {}

impl ParseItem<ItemImpl> for Jobs {
    fn parse_item(&mut self, item: &ItemImpl, paths: &Paths) -> syn::Result<()> {
        let self_ty = (*item.self_ty).clone();
        let self_ident = self_ty_ident(&self_ty)?;
        let self_name = LitStr::new(&self_ident.to_string(), self_ident.span());

        self.context = Some(JobContext {
            self_ty,
            self_name,
            paths: paths.clone(),
        });

        Ok(())
    }
}

impl ParseMethod for Jobs {
    fn parse_method(
        &mut self,
        method: &mut ImplItemFn,
    ) -> syn::Result<Option<upwell_macros_core::client::ClientMethod>> {
        let Some(pos) = method.attrs.iter().position(|a| a.path().is_ident("job")) else {
            return Ok(None);
        };

        let attr = method.attrs.remove(pos);

        // `parse_item` runs before the method walk, so the context is always present.
        let cx = self
            .context
            .as_ref()
            .expect("Jobs::parse_item runs before parse_method");

        let index = self.blocks.len();
        let block = generate_job(&cx.self_ty, &cx.self_name, method, &attr, index, &cx.paths)?;

        self.blocks.push(block);

        // Jobs contribute no client surface.
        Ok(None)
    }
}

impl ToTokens for Jobs {
    fn to_tokens(&self, out: &mut TokenStream) {
        if self.blocks.is_empty() {
            return;
        }

        let blocks = &self.blocks;

        out.extend(quote! {
            const _: () = {
                #(#blocks)*
            };
        });
    }
}

/// Whether a parameter type is the per-run `JobRunContext`, matched by its final path segment
/// (`JobRunContext`, `jobs::JobRunContext`, `upwell::jobs::JobRunContext`, …). A context
/// parameter is fed the threaded run context rather than resolved from the DI container.
fn is_run_context(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };

    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "JobRunContext")
}

/// The parsed `#[job(..)]` attribute: the schedule plus any execution options.
struct JobArgs {
    kind: JobKind,
    schedule: LitStr,
    /// A `JobOptions { .. }` literal built from the option keys, ready to drop into the
    /// emitted descriptor.
    options: TokenStream,
}

/// The raw option keys accumulated while parsing, before they are lowered to a `JobOptions`
/// literal.
#[derive(Default)]
struct RawOptions {
    run_on_startup: bool,
    timeout: Option<LitStr>,
    jitter: Option<LitStr>,
    max_runtime: Option<LitStr>,
    overlap: Option<Ident>,
    timezone: Option<Ident>,
}

/// Parses the `#[job(..)]` attribute: the mandatory `every = ".."` / `cron = ".."` schedule
/// first, then any comma-separated execution options.
fn parse_job_args(attr: &syn::Attribute, paths: &Paths) -> syn::Result<JobArgs> {
    let Meta::List(list) = &attr.meta else {
        return Err(syn::Error::new_spanned(
            attr,
            "expected #[job(every = \"..\")] or #[job(cron = \"..\")]",
        ));
    };

    list.parse_args_with(|input: ParseStream| {
        let key: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let schedule: LitStr = input.parse()?;

        let kind = match key.to_string().as_str() {
            "every" => JobKind::Interval,
            "cron" => JobKind::Cron,

            _ => return Err(syn::Error::new_spanned(&key, "expected `every` or `cron`")),
        };

        let mut raw = RawOptions::default();

        while !input.is_empty() {
            input.parse::<Token![,]>()?;

            // A trailing comma is allowed.
            if input.is_empty() {
                break;
            }

            parse_option(input, &mut raw)?;
        }

        Ok(JobArgs {
            kind,
            schedule,
            options: build_options(&raw, paths)?,
        })
    })
}

/// Parses one `key = value` (or bare flag) execution option into `raw`.
fn parse_option(input: ParseStream, raw: &mut RawOptions) -> syn::Result<()> {
    let key: Ident = input.parse()?;
    let name = key.to_string();

    // `run_on_startup` may be a bare flag or `run_on_startup = true`.
    if name == "run_on_startup" {
        raw.run_on_startup = if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;

            input.parse::<syn::LitBool>()?.value
        } else {
            true
        };

        return Ok(());
    }

    input.parse::<Token![=]>()?;

    match name.as_str() {
        "timeout" => raw.timeout = Some(input.parse()?),
        "jitter" => raw.jitter = Some(input.parse()?),
        "max_runtime" => raw.max_runtime = Some(input.parse()?),
        "overlap" => raw.overlap = Some(input.parse()?),
        "tz" | "timezone" => raw.timezone = Some(input.parse()?),

        _ => {
            return Err(syn::Error::new_spanned(
                &key,
                "expected one of: run_on_startup, timeout, jitter, max_runtime, overlap, tz",
            ));
        }
    }

    Ok(())
}

/// Lowers the accumulated [`RawOptions`] into a const `JobOptions { .. }` literal.
fn build_options(raw: &RawOptions, paths: &Paths) -> syn::Result<TokenStream> {
    let job_options = paths.plugin("JobOptions");
    let overlap_policy = paths.plugin("OverlapPolicy");
    let timezone_ty = paths.plugin("JobTimezone");

    let run_on_startup = raw.run_on_startup;
    let timeout = optional_duration(raw.timeout.as_ref())?;
    let jitter = optional_duration(raw.jitter.as_ref())?;
    let max_runtime = optional_duration(raw.max_runtime.as_ref())?;

    // The overlap / timezone values are the enum variant idents verbatim
    // (`overlap = CancelPrevious`, `tz = Local`); an unknown variant is a compile error at the
    // emitted path, so new variants need no macro change.
    let overlap = match &raw.overlap {
        Some(variant) => quote! { #overlap_policy::#variant },
        None => quote! { #overlap_policy::Skip },
    };

    let timezone = match &raw.timezone {
        Some(variant) => quote! { ::core::option::Option::Some(#timezone_ty::#variant) },
        None => quote! { ::core::option::Option::None },
    };

    Ok(quote! {
        #job_options {
            run_on_startup: #run_on_startup,
            timeout: #timeout,
            jitter: #jitter,
            overlap: #overlap,
            max_runtime: #max_runtime,
            timezone: #timezone,
        }
    })
}

/// Parses an optional humantime duration literal into a const `Option<Duration>` token stream.
/// An invalid duration is a compile error, so a misconfigured `#[job]` never reaches runtime.
fn optional_duration(lit: Option<&LitStr>) -> syn::Result<TokenStream> {
    let Some(lit) = lit else {
        return Ok(quote! { ::core::option::Option::None });
    };

    let parsed = humantime::parse_duration(&lit.value())
        .map_err(|error| syn::Error::new_spanned(lit, format!("invalid duration: {error}")))?;

    let nanos = parsed.as_nanos();
    let nanos =
        u64::try_from(nanos).map_err(|_| syn::Error::new_spanned(lit, "duration is too large"))?;

    Ok(quote! {
        ::core::option::Option::Some(::core::time::Duration::from_nanos(#nanos))
    })
}

/// Parses and emits one `#[job]` method's erased call and its `JobDescriptor` (appended to the
/// global `JOBS` slice). `index` disambiguates multiple jobs on one type.
fn generate_job(
    self_ty: &Type,
    name: &LitStr,
    method: &ImplItemFn,
    attr: &syn::Attribute,
    index: usize,
    paths: &Paths,
) -> syn::Result<TokenStream> {
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "#[job] methods must be async",
        ));
    }

    let JobArgs {
        kind,
        schedule,
        options,
    } = parse_job_args(attr, paths)?;

    let mut takes_self = false;
    let mut param_types: Vec<Type> = Vec::new();

    for arg in &method.sig.inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                if receiver.reference.is_none() || receiver.mutability.is_some() {
                    return Err(syn::Error::new_spanned(
                        receiver,
                        "a #[job] receiver must be `&self`",
                    ));
                }

                takes_self = true;
            }

            FnArg::Typed(typed) => param_types.push((*typed.ty).clone()),
        }
    }

    if !takes_self {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "#[job] methods must take `&self`",
        ));
    }

    let is_result = attr::result_type_args(&method.sig.output).is_some();

    let job_descriptor = paths.plugin("JobDescriptor");
    let job_outcome = paths.plugin("JobOutcome");
    let job_run_context = paths.plugin("JobRunContext");
    let schedule_kind = paths.plugin("ScheduleKind");
    let jobs_slice = paths.plugin("JOBS");
    let distributed_slice = paths.plugin("linkme::distributed_slice");
    let linkme_crate = paths.plugin("linkme");
    let root_resolver = paths.core("RootResolver");
    let type_descriptor = paths.core("TypeDescriptor");

    let method_ident = &method.sig.ident;
    let source = upwell_macros_core::source::item_source(
        method_ident.span(),
        &upwell_macros_core::source::associated_item(self_ty, method_ident),
        paths,
    );
    let kind_variant = format_ident!("{}", kind.variant());

    let call_fn = format_ident!("__upwell_job_{index}_call");
    let descriptor_static = format_ident!("__UPWELL_JOB_{index}");

    let job_name = LitStr::new(
        &format!("{}::{}", name.value(), method_ident),
        method_ident.span(),
    );

    let arg_idents: Vec<Ident> = (0..param_types.len())
        .map(|i| format_ident!("__a{i}"))
        .collect();

    let box_err = quote! {
        |__e| ::std::boxed::Box::new(__e)
            as ::std::boxed::Box<dyn ::std::error::Error + ::core::marker::Send + ::core::marker::Sync>
    };

    // Each parameter is resolved from the container per run — except a `JobRunContext`, which is
    // the per-run context threaded through the call rather than a DI dependency.
    let arg_resolvers = arg_idents.iter().zip(&param_types).map(|(ident, ty)| {
        if is_run_context(ty) {
            quote! { let #ident = _cx.clone(); }
        } else {
            quote! {
                let #ident = __root
                    .extract::<#ty>()
                    .await
                    .map_err(#box_err)?;
            }
        }
    });

    // Normalize the method's return into a `JobOutcome`: a `Result` funnels its error into the
    // boxed job error, anything else is discarded and reported as success.
    let normalize = if is_result {
        quote! { __out.map(|_| ()).map_err(#box_err) }
    } else {
        quote! {
            {
                let _ = __out;

                ::core::result::Result::Ok(())
            }
        }
    };

    Ok(quote! {
        fn #call_fn(
            __root: #root_resolver,
            _cx: #job_run_context,
        ) -> ::core::pin::Pin<
            ::std::boxed::Box<
                dyn ::core::future::Future<Output = #job_outcome> + ::core::marker::Send + 'static,
            >,
        > {
            ::std::boxed::Box::pin(async move {
                let __receiver = __root
                    .component::<#self_ty>()
                    .map_err(#box_err)?;

                #(#arg_resolvers)*

                let __out = __receiver.#method_ident(#(#arg_idents),*).await;

                #normalize
            })
        }

        #[#distributed_slice(#jobs_slice)]
        #[linkme(crate = #linkme_crate)]
        static #descriptor_static: #job_descriptor = #job_descriptor {
            name: #job_name,
            component_ty: #type_descriptor::of::<#self_ty>(#name),
            schedule: #schedule,
            kind: #schedule_kind::#kind_variant,
            call: #call_fn,
            options: #options,
            source: #source,
        };
    })
}
