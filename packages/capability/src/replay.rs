//! The replaying server `#[capability]` emits beside a recorded trait:
//! `<Trait>Replay`, which answers every call from a script, and its
//! `<Trait>Restore` hook when a method is redacted.
//!
//! A trait with an associated type, a callback or an unrecorded method gets
//! none: its replay needs the device's own state, so a composition writes it.

use super::recorder::{Bounds, optional, with_body};
use super::{Arg, Keys, Kind, Media, Method, Role};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, ItemTrait, TraitItem, Type};

pub(super) fn expand(item: &ItemTrait, id: u8, methods: &[Method]) -> TokenStream {
    let served = methods.iter().all(|method| {
        !matches!(method.kind, Kind::Unrecorded(_))
            && method.args.iter().all(|arg| arg.role != Role::Callback)
    });
    let typed = item
        .items
        .iter()
        .any(|entry| matches!(entry, TraitItem::Type(_)));
    if !served || typed {
        return TokenStream::new();
    }
    let journal = quote!(::device_api::journal);
    let name = &item.ident;
    let vis = &item.vis;
    let replay = format_ident!("{}Replay", name);
    let restore = format_ident!("{}Restore", name);

    let mut puts = Bounds::default();
    let mut gets = Bounds::default();
    let mut media: Option<TokenStream> = None;
    let mut hooks = Vec::new();
    let mut bodies = Vec::new();
    let mut sinks = 0usize;
    for (operation, method) in methods.iter().enumerate() {
        let operation = operation as u8;
        let id = quote!(#journal::MethodId { capability: #id, operation: #operation });
        let call = &method.name;
        let identity: Vec<&Arg> = method.args_in(Role::Identity).collect();
        let names: Vec<&Ident> = identity.iter().map(|arg| &arg.name).collect();
        let returns = &method.returns;
        let open = quote! {
            let mut script = self.script.borrow_mut();
            let script = &mut *script;
        };
        if method.redacted {
            let types = identity.iter().map(|arg| &arg.ty);
            let doc = format!(
                "Answer `{call}` from the open call's record, whose secret the recording replaced."
            );
            hooks.push(quote! {
                #[doc = #doc]
                fn #call(&mut self, #(#names: &#types,)*) -> #returns;
            });
            bodies.push(with_body(
                method,
                quote! {
                    #open
                    script.begin(#id);
                    let value = #restore::#call(script, #(&#names,)*);
                    script.end();
                    value
                },
            ));
            continue;
        }
        let put_args = |puts: &mut Bounds| {
            for arg in &identity {
                puts.put(&arg.ty);
            }
            quote!(#(#journal::Put::put(script, &#names);)*)
        };
        let aged = quote! {
            #journal::Aging::aged(value, script.pass().now_ms.saturating_sub(at))
        };
        let body = match &method.kind {
            Kind::Observe(Keys::Asked) => {
                let args = put_args(&mut puts);
                gets.put(returns);
                quote! {
                    #open
                    script.begin(#id);
                    #args
                    script.answer();
                    let value: #returns = #journal::Get::get(script);
                    script.end();
                    value
                }
            }
            Kind::Observe(Keys::All(_)) => {
                let args = put_args(&mut puts);
                gets.put(returns);
                quote! {
                    #open
                    script.begin(#id);
                    #args
                    let at = script.observed();
                    let value: #returns = #journal::Get::get(script);
                    script.end();
                    #aged
                }
            }
            Kind::Observe(Keys::Unkeyed) => match returns {
                Type::Array(array) => {
                    let element = &*array.elem;
                    puts.put(&syn::parse_quote!(u8));
                    gets.put(element);
                    quote! {
                        #open
                        ::core::array::from_fn(|index| {
                            script.begin(#id);
                            #journal::Put::put(script, &(index as u8));
                            let at = script.observed();
                            let value: #element = #journal::Get::get(script);
                            script.end();
                            #aged
                        })
                    }
                }
                _ => {
                    gets.put(returns);
                    quote! {
                        #open
                        script.begin(#id);
                        let at = script.observed();
                        let value: #returns = #journal::Get::get(script);
                        script.end();
                        #aged
                    }
                }
            },
            Kind::Take { .. } => {
                let args = put_args(&mut puts);
                let out = method.args_in(Role::Out).next();
                match (out, optional(returns)) {
                    (Some(out), _) => {
                        let out = &out.name;
                        gets.put(&syn::parse_quote!(bool));
                        quote! {
                            #open
                            let mut read = 0;
                            loop {
                                script.begin(#id);
                                #args
                                script.answer();
                                let more: bool = #journal::Get::get(script);
                                read += script.fill(&mut #out[read..]);
                                script.end();
                                if !more {
                                    break read;
                                }
                            }
                        }
                    }
                    (None, Some(item)) => {
                        gets.put(item);
                        quote! {
                            #open
                            script.begin(#id);
                            #args
                            let value = match script.answered() {
                                true => {
                                    let item: #item = #journal::Get::get(script);
                                    ::core::option::Option::Some(item)
                                }
                                false => ::core::option::Option::None,
                            };
                            script.end();
                            value
                        }
                    }
                    (None, None) => {
                        gets.put(returns);
                        quote! {
                            #open
                            script.begin(#id);
                            #args
                            script.answer();
                            let value: #returns = #journal::Get::get(script);
                            script.end();
                            value
                        }
                    }
                }
            }
            Kind::Request { .. } | Kind::Command => {
                let args = put_args(&mut puts);
                let inputs: Vec<&Ident> =
                    method.args_in(Role::Input).map(|arg| &arg.name).collect();
                if !inputs.is_empty() {
                    puts.put(&syn::parse_quote!([u8]));
                }
                gets.put(returns);
                quote! {
                    #open
                    script.begin(#id);
                    #args
                    #(#journal::Put::put(script, &#inputs[..]);)*
                    script.answer();
                    let value: #returns = #journal::Get::get(script);
                    script.end();
                    value
                }
            }
            Kind::Sink { .. } => {
                let sink = sinks;
                sinks += 1;
                puts.put(&syn::parse_quote!(u8));
                let args = put_args(&mut puts);
                let lengths: Vec<&Ident> =
                    method.args_in(Role::Input).map(|arg| &arg.name).collect();
                if !lengths.is_empty() {
                    puts.put(&syn::parse_quote!(usize));
                }
                gets.put(returns);
                quote! {
                    #open
                    self.sampled.enter(script.pass().number);
                    let attempt = self.sampled.attempt(#sink);
                    script.begin(#id);
                    #journal::Put::put(script, &attempt);
                    #args
                    #(#journal::Put::put(script, &#lengths.len());)*
                    let value: #returns = if script.answered() {
                        #journal::Get::get(script)
                    } else if let ::core::option::Option::Some(value) =
                        <#returns as #journal::Accepted>::accepted()
                    {
                        value
                    } else {
                        script.answer();
                        #journal::Get::get(script)
                    };
                    script.end();
                    value
                }
            }
            Kind::Media(verb) => {
                let at: Vec<&Arg> = method.args_in(Role::At).collect();
                let names: Vec<&Ident> = at.iter().map(|arg| &arg.name).collect();
                let types: Vec<&Type> = at.iter().map(|arg| &arg.ty).collect();
                let (place, place_ty) = match (names.as_slice(), types.as_slice()) {
                    ([one], [ty]) => (quote!(#one), quote!(#ty)),
                    _ => (quote!((#(#names,)*)), quote!((#(#types,)*))),
                };
                media = Some(quote!(#journal::Stored<#place_ty, #returns>));
                let Some(what) = method.args_in(verb.role()).next() else {
                    continue;
                };
                let what = &what.name;
                let serve = match verb {
                    Media::Read => quote!(read(script, #id, #place, &mut #what[..])),
                    Media::Write => quote!(write(script, #id, #place, &#what[..])),
                    Media::Erase => quote!(erase(script, #id, #place, #what)),
                };
                quote! {
                    #open
                    #journal::Stored::#serve
                }
            }
            Kind::Derived => {
                let derived = format_ident!("derived_{}", call);
                let passed = method.args.iter().map(|arg| &arg.name);
                quote!(#derived(self, #(#passed),*))
            }
            Kind::Unrecorded(_) => continue,
        };
        bodies.push(with_body(method, body));
    }

    let (puts, gets) = (&puts.puts, &gets.puts);
    let media_bound = media.map(|bound| quote!(+ #bound));
    let hook_bound = (!hooks.is_empty()).then(|| quote!(+ #restore));
    let hook_trait = (!hooks.is_empty()).then(|| {
        let doc = format!(
            "How a replay answers the redacted methods of [`{name}`]. Each method is called with its call open."
        );
        quote! {
            #[doc = #doc]
            #vis trait #restore {
                #(#hooks)*
            }
        }
    });
    let doc = format!(
        "A [`{name}`] replayed from a script: it holds no device, and answers every call with what the recording gave."
    );
    let (sampled_field, sampled_init) = match sinks {
        0 => (quote!(), quote!()),
        _ => (
            quote!(sampled: #journal::Sampled,),
            quote!(sampled: ::core::default::Default::default(),),
        ),
    };
    quote! {
        #hook_trait

        #[doc = #doc]
        #vis struct #replay<H> {
            script: H,
            #sampled_field
        }

        impl<H> #replay<H> {
            pub fn new(script: H) -> Self {
                Self {
                    script,
                    #sampled_init
                }
            }
        }

        impl<H, J> #name for #replay<H>
        where
            H: ::core::ops::Deref<Target = ::core::cell::RefCell<J>>,
            J: #journal::Script
                #(+ #journal::Put<#puts>)*
                #(+ #journal::Get<#gets>)*
                #media_bound
                #hook_bound,
        {
            #(#bodies)*
        }
    }
}
