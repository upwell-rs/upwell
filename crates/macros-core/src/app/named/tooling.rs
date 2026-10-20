use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::{Expr, Ident};

use crate::paths::Paths;

/// Expands the target-local tooling identity and preparation entry.
pub(super) fn expand(
    ident: &Ident,
    application_name: &Expr,
    paths: &Paths,
) -> syn::Result<TokenStream> {
    let application_name = match application_name {
        Expr::Lit(expression) => match &expression.lit {
            syn::Lit::Str(name) => name.clone(),
            _ => return Err(literal_name_error(application_name)),
        },
        _ => return Err(literal_name_error(application_name)),
    };
    let probe_target_identity = paths.core("tooling::ProbeTargetIdentity");
    let ident_name = ident.to_string();
    let file = quote_spanned!(ident.span()=> ::core::file!());
    let line = quote_spanned!(ident.span()=> ::core::line!());
    let column = quote_spanned!(ident.span()=> ::core::column!());
    let source_location = paths.core("tooling::SourceLocation");
    let probe_envelope = paths.core("tooling::ProbeEnvelope");
    let identity_error = paths.core("tooling::IdentityValidationError");
    let catch_probe_panic = paths.core("__private::catch_probe_panic");
    let initial = paths.core("Initial");
    #[cfg(feature = "cli")]
    let run_probe = {
        let execution_mode = paths.core("ExecutionMode");
        let probe_host = paths.core("__private::probe_bootstrapped_host");

        quote! {
            let context = Self::__upwell_parse_cli(
                [#application_name],
                #execution_mode::Tooling,
                false,
            ).map(|(context, _, _)| context);

            #probe_host::<#ident<#initial>>(identity, context).await
        }
    };
    #[cfg(not(feature = "cli"))]
    let run_probe = {
        let probe_host = paths.core("__private::probe_host");

        quote!(#probe_host::<#ident<#initial>>(identity).await)
    };

    Ok(quote! {
        impl #ident<#initial> {
            /// Prepares and projects this application for an explicitly selected Cargo target.
            ///
            /// This target-local seam is independent of Clap. A tooling-only thin binary may call
            /// it directly and emit the returned envelope through the response-file contract.
            /// Its unwind boundary does not replace an embedding process's global panic hook, so
            /// callers requiring panic-payload secrecy must use the generated dedicated process
            /// invocation instead.
            ///
            /// # Errors
            ///
            /// Returns an identity error when generated declaration identity is incomplete.
            #[doc(hidden)]
            pub async fn tooling_probe(
                target: #probe_target_identity,
            ) -> ::core::result::Result<#probe_envelope, #identity_error> {
                let identity = target.document_identity(
                    (#application_name).to_string(),
                    #source_location {
                        file: #file.to_string(),
                        line: ::core::option::Option::Some(#line),
                        column: ::core::option::Option::Some(#column),
                        rust_path: ::core::option::Option::Some(
                            ::std::format!("{}::{}", ::core::module_path!(), #ident_name),
                        ),
                    },
                )?;
                let panic_identity = identity.clone();
                let probe = async move { #run_probe };

                Ok(#catch_probe_panic(panic_identity, probe).await)
            }

            /// Internal generated alias used by the process runner and crate-local tests.
            #[doc(hidden)]
            pub(crate) async fn __upwell_tooling_probe(
                target: #probe_target_identity,
            ) -> ::core::result::Result<#probe_envelope, #identity_error> {
                Self::tooling_probe(target).await
            }
        }
    })
}

fn literal_name_error(application_name: &Expr) -> syn::Error {
    syn::Error::new_spanned(
        application_name,
        "named apps with tooling support require a string literal `name`",
    )
}
