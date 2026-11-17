//! The recording wrapper `#[capability]` emits beside a recorded trait:
//! `<Trait>Recorder`, its `<Trait>Redact` hook when a method is redacted,
//! and the trait for `&mut T`.

use super::{Arg, Errors, Keys, Kind, Media, Method, Role, key};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, GenericArgument, Ident, ItemTrait, Pat, PathArguments, TraitItem, Type};

/// The most sinks a capability counts; `device_api::journal::Sampled::SINKS`.
const SINKS: usize = 2;
/// The most repeating requests a capability counts;
/// `device_api::journal::Sampled::REPEATS`.
const REPEATS: usize = 2;
/// One bit of `Sampled` per observe.
const OBSERVES: usize = 32;

#[derive(Default)]
pub(super) struct Bounds {
    pub(super) puts: Vec<Type>,
    seen: Vec<String>,
}

impl Bounds {
    pub(super) fn put(&mut self, ty: &Type) {
        if !self.seen.contains(&key(ty)) {
            self.seen.push(key(ty));
            self.puts.push(ty.clone());
        }
    }
}

enum Shape {
    Value,
    /// A `[T; N]`: each element is recorded alone, under its index.
    Elements(Type),
    /// A `&'static [T]`: its length, then each element.
    Slice(Type),
    Keyed(syn::Expr, Type),
}

struct Observe<'a> {
    method: &'a Method,
    operation: u8,
    bit: u32,
    shape: Shape,
}

pub(super) fn expand(
    item: &ItemTrait,
    id: u8,
    methods: &[Method],
    errors: &mut Errors,
) -> TokenStream {
    let journal = quote!(::device_api::journal);
    let name = &item.ident;
    let vis = &item.vis;
    let recorder = format_ident!("{}Recorder", name);
    let observed = format_ident!("{}Observed", name);
    let redact = format_ident!("{}Redact", name);
    let types: Vec<&Ident> = item
        .items
        .iter()
        .filter_map(|entry| match entry {
            TraitItem::Type(ty) => Some(&ty.ident),
            _ => None,
        })
        .collect();

    let mut bounds = Bounds::default();
    let mut observes = Vec::new();
    let mut sinks = 0usize;
    let mut refreshes = false;
    let mut media: Option<TokenStream> = None;
    let mut hooks = Vec::new();
    let mut bodies = Vec::new();
    let mut lasts: Vec<(Ident, TokenStream)> = Vec::new();
    let repeating = methods
        .iter()
        .any(|method| matches!(method.kind, Kind::Request { repeats: true, .. }));
    for (operation, method) in methods.iter().enumerate() {
        let operation = operation as u8;
        let id = quote!(#journal::MethodId { capability: #id, operation: #operation });
        let call = &method.name;
        let passed: Vec<&Ident> = method.args.iter().map(|arg| &arg.name).collect();
        let identity: Vec<&Arg> = method.args_in(Role::Identity).collect();
        let kept = identity.iter().map(|arg| {
            let name = &arg.name;
            quote!(::core::clone::Clone::clone(&#name))
        });
        let kept = quote!(let recorded = (#(#kept,)*););
        let slots: Vec<syn::Index> = (0..identity.len()).map(syn::Index::from).collect();
        let hook = |hooks: &mut Vec<TokenStream>, value: &Type| {
            let types = identity.iter().map(|arg| &arg.ty);
            let names = identity.iter().map(|arg| &arg.name);
            let doc = format!(
                "Write `{call}`'s record body with its secret replaced: its arguments, then what it gave."
            );
            hooks.push(quote! {
                #[doc = #doc]
                fn #call(&mut self, #(#names: &#types,)* value: &#value);
            });
        };
        let records_args = !matches!(
            method.kind,
            Kind::Media(_) | Kind::Derived | Kind::Unrecorded(_)
        );
        let put_args = if method.redacted || !records_args {
            quote!()
        } else {
            for arg in &identity {
                bounds.put(&arg.ty);
            }
            quote!(#(#journal::Put::put(journal, &recorded.#slots);)*)
        };
        let refresh = match method.kind {
            Kind::Take { refreshes: true }
            | Kind::Request {
                refreshes: true, ..
            }
            | Kind::Sink { refreshes: true } => quote!(self.refresh();),
            _ => quote!(),
        };
        refreshes |= !refresh.is_empty();
        let settle = match refresh.is_empty() || !repeating {
            true => quote!(),
            false => quote!(self.state.borrow_mut().sampled.settle();),
        };
        let open = quote! {
            let mut journal = self.journal.borrow_mut();
            let journal = &mut *journal;
        };
        let body = match &method.kind {
            Kind::Observe(keys) => {
                let bit = observes.len() as u32;
                let shape = match keys {
                    Keys::Asked => {
                        let (Some(asked), false) = (identity.first(), method.redacted) else {
                            errors.at(call, "an observe keyed by what VANA asks can't be redacted");
                            continue;
                        };
                        bounds.put(&asked.ty);
                        bounds.put(&method.returns);
                        bodies.push(with_body(
                            method,
                            quote! {
                                #kept
                                let value = self.dev.#call(#(#passed),*);
                                #open
                                journal.begin(#id);
                                #put_args
                                #journal::Put::put(journal, &value);
                                journal.end();
                                value
                            },
                        ));
                        continue;
                    }
                    Keys::All(keys) => Shape::Keyed(keys.clone(), identity[0].ty.clone()),
                    Keys::Unkeyed => match &method.returns {
                        Type::Array(array) => Shape::Elements((*array.elem).clone()),
                        Type::Reference(slice) => match &*slice.elem {
                            Type::Slice(slice) => Shape::Slice((*slice.elem).clone()),
                            _ => Shape::Value,
                        },
                        _ => Shape::Value,
                    },
                };
                if method.redacted {
                    if !matches!(shape, Shape::Value) {
                        errors.at(call, "a redacted observe returns one value");
                    }
                    hook(&mut hooks, &method.returns);
                } else {
                    match &shape {
                        Shape::Value => bounds.put(&method.returns),
                        Shape::Elements(element) => {
                            bounds.put(&syn::parse_quote!(u8));
                            bounds.put(element);
                        }
                        Shape::Slice(element) => {
                            bounds.put(&syn::parse_quote!(usize));
                            bounds.put(element);
                        }
                        Shape::Keyed(_, key) => {
                            bounds.put(key);
                            bounds.put(&method.returns);
                        }
                    }
                }
                let observe = Observe {
                    method,
                    operation,
                    bit,
                    shape,
                };
                bodies.push(with_body(method, read(&observe)));
                observes.push(observe);
                continue;
            }
            Kind::Take { .. } => {
                let out = method.args_in(Role::Out).next();
                let item = optional(&method.returns);
                let write = if method.redacted {
                    hook(&mut hooks, item.unwrap_or(&method.returns));
                    quote! {
                        journal.begin(#id);
                        #redact::#call(journal, #(&recorded.#slots,)* item);
                        journal.end();
                    }
                } else if let Some(out) = out {
                    let out = &out.name;
                    bounds.put(&syn::parse_quote!(bool));
                    bounds.put(&syn::parse_quote!([u8]));
                    quote! {
                        let read = &#out[..::core::cmp::min(*item, #out.len())];
                        let mut parts = read.chunks(#journal::PART).peekable();
                        loop {
                            let part: &[u8] = parts.next().unwrap_or(&[]);
                            let more = parts.peek().is_some();
                            journal.begin(#id);
                            #put_args
                            #journal::Put::put(journal, &more);
                            #journal::Put::put(journal, part);
                            journal.end();
                            if !more {
                                break;
                            }
                        }
                    }
                } else {
                    bounds.put(item.unwrap_or(&method.returns));
                    quote! {
                        journal.begin(#id);
                        #put_args
                        #journal::Put::put(journal, item);
                        journal.end();
                    }
                };
                let recorded = if item.is_some() {
                    quote!(if let ::core::option::Option::Some(item) = &value { #open #write #settle })
                } else {
                    quote!({ let item = &value; #open #write #settle })
                };
                quote! {
                    #kept
                    let value = self.dev.#call(#(#passed),*);
                    #recorded
                    #refresh
                    value
                }
            }
            Kind::Request { .. } | Kind::Command => {
                let inputs: Vec<&Ident> =
                    method.args_in(Role::Input).map(|arg| &arg.name).collect();
                let write = if method.redacted {
                    hook(&mut hooks, &method.returns);
                    quote!(#redact::#call(journal, #(&recorded.#slots,)* &value);)
                } else {
                    bounds.put(&method.returns);
                    if !inputs.is_empty() {
                        bounds.put(&syn::parse_quote!([u8]));
                    }
                    quote! {
                        #put_args
                        #(#journal::Put::put(journal, &#inputs[..]);)*
                        #journal::Put::put(journal, &value);
                    }
                };
                if let Kind::Request { repeats: true, .. } = method.kind {
                    let repeat = lasts.len();
                    let last = format_ident!("last_{}", call);
                    let types = identity.iter().map(|arg| &arg.ty);
                    let returns = &method.returns;
                    lasts.push((last.clone(), quote!(((#(#types,)*), #returns))));
                    quote! {
                        #kept
                        let value = self.dev.#call(#(#passed),*);
                        let (repeat, before) = {
                            let pass = self.journal.borrow().pass();
                            let mut state = self.state.borrow_mut();
                            state.sampled.enter(pass.number);
                            match &state.#last {
                                ::core::option::Option::Some((args, gave)) if *args == recorded => {
                                    (*gave == value, state.sampled.repeats(#repeat))
                                }
                                _ => (false, 0),
                            }
                        };
                        {
                            #open
                            journal.begin(#id);
                            #write
                            match repeat {
                                true => journal.hold(),
                                false => journal.end(),
                            }
                        }
                        #refresh
                        let elided = repeat && self.journal.borrow_mut().release();
                        let mut state = self.state.borrow_mut();
                        if elided {
                            state.sampled.repeat(#repeat);
                        } else {
                            if before > 0 {
                                self.journal.borrow_mut().repeated(#id, before);
                            }
                            state.sampled.settle();
                            state.#last = ::core::option::Option::Some((
                                recorded,
                                ::core::clone::Clone::clone(&value),
                            ));
                        }
                        value
                    }
                } else {
                    quote! {
                        #kept
                        let value = self.dev.#call(#(#passed),*);
                        {
                            #open
                            journal.begin(#id);
                            #write
                            journal.end();
                        }
                        #settle
                        #refresh
                        value
                    }
                }
            }
            Kind::Sink { .. } => {
                if method.redacted {
                    errors.at(
                        call,
                        "a sink's output is not recorded, so it has nothing to redact",
                    );
                }
                let sink = sinks;
                sinks += 1;
                bounds.put(&syn::parse_quote!(u8));
                bounds.put(&method.returns);
                let lengths: Vec<&Ident> =
                    method.args_in(Role::Input).map(|arg| &arg.name).collect();
                if !lengths.is_empty() {
                    bounds.put(&syn::parse_quote!(usize));
                }
                quote! {
                    #kept
                    let value = self.dev.#call(#(#passed),*);
                    {
                        #open
                        let attempt = {
                            let mut state = self.state.borrow_mut();
                            state.sampled.enter(journal.pass().number);
                            state.sampled.attempt(#sink)
                        };
                        if #journal::Refusal::refused(&value) {
                            journal.begin(#id);
                            #journal::Put::put(journal, &attempt);
                            #put_args
                            #(#journal::Put::put(journal, &#lengths.len());)*
                            #journal::Put::put(journal, &value);
                            journal.end();
                            #settle
                        }
                    }
                    #refresh
                    value
                }
            }
            Kind::Media(verb) => {
                let at: Vec<&Arg> = method.args_in(Role::At).collect();
                let names: Vec<&Ident> = at.iter().map(|arg| &arg.name).collect();
                let types: Vec<&Type> = at.iter().map(|arg| &arg.ty).collect();
                let (place, place_ty) = match (names.as_slice(), types.as_slice()) {
                    ([one], [ty]) => (quote!(::core::clone::Clone::clone(&#one)), quote!(#ty)),
                    _ => (
                        quote!((#(::core::clone::Clone::clone(&#names),)*)),
                        quote!((#(#types,)*)),
                    ),
                };
                let outcome = &method.returns;
                let bound = quote!(#journal::Media<#place_ty, #outcome>);
                match &media {
                    Some(first) if first.to_string() != bound.to_string() => errors.at(
                        call,
                        "a capability's media methods take one address and give one outcome",
                    ),
                    _ => media = Some(bound),
                }
                let Some(what) = method.args_in(verb.role()).next() else {
                    continue;
                };
                let what = &what.name;
                let record = match verb {
                    Media::Read => quote!(read(journal, #id, at, &#what[..], &value)),
                    Media::Write => quote!(wrote(journal, #id, at, &#what[..], &value)),
                    Media::Erase => {
                        quote!(erased(journal, #id, at, recorded_length, &value))
                    }
                };
                let length = match verb {
                    Media::Erase => quote!(let recorded_length: usize = #what;),
                    _ => quote!(),
                };
                quote! {
                    let at = #place;
                    #length
                    let value = self.dev.#call(#(#passed),*);
                    {
                        #open
                        #journal::Media::#record;
                    }
                    value
                }
            }
            Kind::Derived => {
                let derived = format_ident!("derived_{}", call);
                quote!(#derived(self, #(#passed),*))
            }
            Kind::Unrecorded(_) => quote!(self.dev.#call(#(#passed),*)),
        };
        let body = with_body(method, body);
        bodies.push(match method.kind {
            Kind::Sink { .. } => quote!(#[inline(never)] #body),
            _ => body,
        });
    }
    if sinks > SINKS {
        errors.at(name, format!("a capability counts at most {SINKS} sinks"));
    }
    if lasts.len() > REPEATS {
        errors.at(
            name,
            format!("a capability counts at most {REPEATS} repeating requests"),
        );
    }
    let last_fields = lasts
        .iter()
        .map(|(last, ty)| quote!(#last: ::core::option::Option<#ty>));
    let last_defaults = lasts
        .iter()
        .map(|(last, _)| quote!(#last: ::core::option::Option::None));
    if observes.len() > OBSERVES {
        errors.at(
            name,
            format!("a capability samples at most {OBSERVES} observes"),
        );
    }

    let fields = observes.iter().map(|observe| {
        let field = &observe.method.name;
        let returns = &observe.method.returns;
        match &observe.shape {
            Shape::Value | Shape::Slice(_) => quote!(#field: #journal::Latest<#returns>),
            Shape::Elements(element) => {
                let Type::Array(array) = returns else {
                    unreachable!()
                };
                let len = &array.len;
                quote!(#field: [#journal::Latest<#element>; #len])
            }
            Shape::Keyed(keys, _) => quote!(#field: [#journal::Latest<#returns>; #keys.len()]),
        }
    });
    let defaults = observes.iter().map(|observe| {
        let field = &observe.method.name;
        match observe.shape {
            Shape::Value | Shape::Slice(_) => quote!(#field: ::core::default::Default::default()),
            _ => quote!(#field: ::core::array::from_fn(|_| ::core::default::Default::default())),
        }
    });
    let samplers: Vec<TokenStream> = observes
        .iter()
        .map(|observe| sampler(observe, id, &redact, &observed))
        .collect();
    let sample_names: Vec<Ident> = observes
        .iter()
        .map(|observe| format_ident!("sample_{}", observe.method.name))
        .collect();
    let bits: Vec<u32> = observes.iter().map(|observe| observe.bit).collect();
    let puts = &bounds.puts;
    let hook_bound = (!hooks.is_empty()).then(|| quote!(+ #redact));
    let media_bound = media.map(|bound| {
        let bound = owned(bound);
        quote!(+ #bound)
    });
    let hook_trait = (!hooks.is_empty()).then(|| {
        let doc = format!(
            "How the secrets of [`{name}`] reach the trace. Each method is called with its record open."
        );
        quote! {
            #[doc = #doc]
            #vis trait #redact {
                #(#hooks)*
            }
        }
    });
    let recorder_doc = format!(
        "A [`{name}`] recorded into a journal: it holds the device, forwards every call and records what the call gave. Its samples live in a [`{observed}`] the composition keeps, so the recorder itself may be made anew each pass."
    );
    let observed_doc = format!(
        "What a [`{recorder}`] keeps between calls: the last recorded value of each observation, and what the pass has sampled."
    );
    let refresh = refreshes.then(|| {
        quote! {
            #[inline(never)]
            fn refresh(&self) {
                let pass = self.journal.borrow().pass();
                let mut state = self.state.borrow_mut();
                let state = &mut *state;
                state.sampled.enter(pass.number);
                #(if state.sampled.has(#bits) {
                    self.#sample_names(state, pass.now_ms);
                })*
            }
        }
    });
    let repeats = lasts.len();
    quote! {
        #hook_trait

        const _: () = assert!(
            #sinks <= #journal::Sampled::SINKS && #repeats <= #journal::Sampled::REPEATS
        );

        #[doc = #observed_doc]
        #vis struct #observed {
            sampled: #journal::Sampled,
            #(#fields,)*
            #(#last_fields,)*
        }

        impl ::core::default::Default for #observed {
            fn default() -> Self {
                Self {
                    sampled: ::core::default::Default::default(),
                    #(#defaults,)*
                    #(#last_defaults,)*
                }
            }
        }

        #[doc = #recorder_doc]
        #vis struct #recorder<D, H, S> {
            dev: D,
            journal: H,
            state: S,
        }

        impl<D, H, S> #recorder<D, H, S> {
            pub fn new(dev: D, journal: H, state: S) -> Self {
                Self { dev, journal, state }
            }

            /// The device, for a caller the trace does not record.
            pub fn unrecorded(&self) -> &D {
                &self.dev
            }

            /// The device, for a caller the trace does not record.
            pub fn unrecorded_mut(&mut self) -> &mut D {
                &mut self.dev
            }
        }

        impl<D, H, S, J> #recorder<D, H, S>
        where
            D: #name,
            H: ::core::ops::Deref<Target = ::core::cell::RefCell<J>>,
            S: ::core::ops::Deref<Target = ::core::cell::RefCell<#observed>>,
            J: #journal::Journal #(+ #journal::Put<#puts>)* #media_bound #hook_bound,
        {
            /// Sample every observation this pass has not sampled yet, and
            /// record those that changed.
            #[inline(never)]
            pub fn sample(&self) {
                let pass = self.journal.borrow().pass();
                let mut state = self.state.borrow_mut();
                let state = &mut *state;
                state.sampled.enter(pass.number);
                #(if !state.sampled.has(#bits) {
                    self.#sample_names(state, pass.now_ms);
                })*
            }

            #refresh

            #(#samplers)*
        }

        impl<D, H, S, J> #name for #recorder<D, H, S>
        where
            D: #name,
            H: ::core::ops::Deref<Target = ::core::cell::RefCell<J>>,
            S: ::core::ops::Deref<Target = ::core::cell::RefCell<#observed>>,
            J: #journal::Journal #(+ #journal::Put<#puts>)* #media_bound #hook_bound,
        {
            #(type #types = D::#types;)*
            #(#bodies)*
        }
    }
}

/// The trait for `&mut T`, so a composition lends a device it keeps, and
/// for `&T` when every method only reads.
pub(super) fn borrowed(item: &ItemTrait) -> TokenStream {
    let name = &item.ident;
    let entries: Vec<TokenStream> = item
        .items
        .iter()
        .filter_map(|entry| match entry {
            TraitItem::Type(ty) => {
                let ident = &ty.ident;
                Some(quote!(type #ident = T::#ident;))
            }
            TraitItem::Fn(function) => {
                let sig = &function.sig;
                let call = &sig.ident;
                let passed = sig.inputs.iter().filter_map(|input| match input {
                    FnArg::Typed(arg) => match &*arg.pat {
                        Pat::Ident(pat) => Some(&pat.ident),
                        _ => None,
                    },
                    FnArg::Receiver(_) => None,
                });
                Some(quote!(#sig { (**self).#call(#(#passed),*) }))
            }
            _ => None,
        })
        .collect();
    let reads_only = item.items.iter().all(|entry| match entry {
        TraitItem::Fn(function) => function
            .sig
            .receiver()
            .is_some_and(|receiver| receiver.mutability.is_none()),
        _ => true,
    });
    let shared = reads_only.then(|| {
        quote! {
            impl<T: #name + ?Sized> #name for &T {
                #(#entries)*
            }
        }
    });
    quote! {
        impl<T: #name + ?Sized> #name for &mut T {
            #(#entries)*
        }
        #shared
    }
}

pub(super) fn with_body(method: &Method, body: TokenStream) -> TokenStream {
    let sig = &method.sig;
    quote! {
        #[allow(unused_variables)]
        #sig {
            #body
        }
    }
}

fn device_call(observe: &Observe<'_>, key: Option<TokenStream>) -> TokenStream {
    let call = &observe.method.name;
    let args = observe.method.args.iter().map(|arg| match arg.role {
        Role::Clock => quote!(now),
        _ => key.clone().unwrap_or_default(),
    });
    quote!(self.dev.#call(#(#args),*))
}

fn sampler(observe: &Observe<'_>, capability: u8, redact: &Ident, observed: &Ident) -> TokenStream {
    let journal = quote!(::device_api::journal);
    let field = &observe.method.name;
    let name = format_ident!("sample_{}", field);
    let (operation, bit) = (observe.operation, observe.bit);
    let id = quote!(#journal::MethodId { capability: #capability, operation: #operation });
    let open = quote! {
        let mut journal = self.journal.borrow_mut();
        let journal = &mut *journal;
        journal.begin(#id);
    };
    let body = match &observe.shape {
        Shape::Value | Shape::Slice(_) => {
            let value = device_call(observe, None);
            let write = match (&observe.shape, observe.method.redacted) {
                (_, true) => quote!(#redact::#field(journal, &value);),
                (Shape::Slice(_), _) => quote! {
                    #journal::Put::put(journal, &value.len());
                    for item in value.iter() {
                        #journal::Put::put(journal, item);
                    }
                },
                _ => quote!(#journal::Put::put(journal, &value);),
            };
            quote! {
                let value = #value;
                if state.#field.changed(::core::clone::Clone::clone(&value), now) {
                    #open
                    #write
                    journal.end();
                }
            }
        }
        Shape::Elements(_) => {
            let value = device_call(observe, None);
            quote! {
                let value = #value;
                for (index, (latest, item)) in state.#field.iter_mut().zip(value).enumerate() {
                    if latest.changed(::core::clone::Clone::clone(&item), now) {
                        #open
                        #journal::Put::put(journal, &(index as u8));
                        #journal::Put::put(journal, &item);
                        journal.end();
                    }
                }
            }
        }
        Shape::Keyed(keys, _) => {
            let value = device_call(observe, Some(quote!(::core::clone::Clone::clone(key))));
            quote! {
                for (latest, key) in state.#field.iter_mut().zip(#keys.iter()) {
                    let value = #value;
                    if latest.changed(::core::clone::Clone::clone(&value), now) {
                        #open
                        #journal::Put::put(journal, key);
                        #journal::Put::put(journal, &value);
                        journal.end();
                    }
                }
            }
        }
    };
    quote! {
        fn #name(&self, state: &mut #observed, now: u64) {
            #body
            state.sampled.mark(#bit);
        }
    }
}

fn read(observe: &Observe<'_>) -> TokenStream {
    let field = &observe.method.name;
    let sample = format_ident!("sample_{}", field);
    let bit = observe.bit;
    let key = observe
        .method
        .args
        .iter()
        .find(|arg| arg.role == Role::Identity)
        .map(|arg| &arg.name);
    let frozen = match &observe.shape {
        Shape::Value | Shape::Slice(_) => {
            let device = device_call(observe, None);
            quote! {
                match state.#field.at(now) {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => #device,
                }
            }
        }
        Shape::Elements(_) => {
            let device = device_call(observe, None);
            quote! {
                ::core::array::from_fn(|index| match state.#field[index].at(now) {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => ::core::clone::Clone::clone(&#device[index]),
                })
            }
        }
        Shape::Keyed(keys, _) => {
            let device = device_call(observe, Some(quote!(#key)));
            quote! {
                match #keys
                    .iter()
                    .position(|known| *known == #key)
                    .and_then(|index| state.#field[index].at(now))
                {
                    ::core::option::Option::Some(value) => value,
                    ::core::option::Option::None => #device,
                }
            }
        }
    };
    quote! {
        let pass = self.journal.borrow().pass();
        let now = pass.now_ms;
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        state.sampled.enter(pass.number);
        if !state.sampled.has(#bit) {
            self.#sample(state, now);
        }
        #frozen
    }
}

pub(super) fn optional(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn owned(tokens: TokenStream) -> TokenStream {
    tokens
        .into_iter()
        .map(|token| match token {
            proc_macro2::TokenTree::Ident(ident) if ident == "Self" => {
                proc_macro2::TokenTree::Ident(Ident::new("D", ident.span()))
            }
            proc_macro2::TokenTree::Group(group) => {
                let mut inner = proc_macro2::Group::new(group.delimiter(), owned(group.stream()));
                inner.set_span(group.span());
                proc_macro2::TokenTree::Group(inner)
            }
            other => other,
        })
        .collect()
}
