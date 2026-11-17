use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Attribute, Error, Expr, FnArg, Ident, ItemTrait, LitStr, Pat, ReturnType, TraitItem,
    TraitItemFn, Type,
};

mod recorder;
mod replay;

const KINDS: &str = "#[observe], #[take], #[request], #[sink], #[media(..)], #[command], #[derived] or #[unrecorded(\"reason\")]";
const ROLES: &str = "#[clock], #[at], #[out], #[input] or #[len]";
const PASSTHROUGH: [&str; 6] = ["doc", "allow", "expect", "warn", "deny", "must_use"];
const CONDITIONAL: [&str; 2] = ["cfg", "cfg_attr"];

/// Declares a device capability: `#[capability(id = N)]`, or
/// `#[capability(id = N, outside = "reason")]` for one that stays out of the
/// recorded world. Beside the trait it emits the capability's table, the
/// trait for `&mut T` and, for a recorded capability, `<Trait>Recorder`.
/// `<Trait>Replay` is emitted too, unless the trait has an associated type,
/// a callback or an unrecorded method.
#[proc_macro_attribute]
pub fn capability(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(item as ItemTrait);
    expand(attr.into(), item)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

enum Keys {
    Unkeyed,
    Asked,
    All(Expr),
}

enum Media {
    Read,
    Write,
    Erase,
}

impl Media {
    fn role(&self) -> Role {
        match self {
            Media::Read => Role::Out,
            Media::Write => Role::Input,
            Media::Erase => Role::Len,
        }
    }
}

enum Kind {
    Observe(Keys),
    Take { refreshes: bool },
    Request { refreshes: bool, repeats: bool },
    Sink { refreshes: bool },
    Media(Media),
    Command,
    Derived,
    Unrecorded(LitStr),
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Identity,
    Clock,
    At,
    Out,
    Input,
    Len,
    Callback,
}

struct Arg {
    name: Ident,
    role: Role,
    ty: Type,
}

struct Declaration {
    kind: Kind,
    redacted: bool,
}

struct Method {
    name: Ident,
    kind: Kind,
    redacted: bool,
    args: Vec<Arg>,
    returns: Type,
    sig: syn::Signature,
}

impl Method {
    fn args_in(&self, role: Role) -> impl Iterator<Item = &Arg> {
        self.args.iter().filter(move |arg| arg.role == role)
    }
}

#[derive(Default)]
struct Errors(Option<Error>);

impl Errors {
    fn push(&mut self, error: Error) {
        match &mut self.0 {
            Some(all) => all.combine(error),
            None => self.0 = Some(error),
        }
    }

    fn at(&mut self, span: &impl Spanned, message: impl core::fmt::Display) {
        self.push(Error::new(span.span(), message));
    }

    fn finish(self) -> syn::Result<()> {
        self.0.map_or(Ok(()), Err)
    }
}

fn expand(attr: TokenStream, mut item: ItemTrait) -> syn::Result<TokenStream> {
    let (id, outside) = header(attr)?;
    let mut errors = Errors::default();
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        errors.at(&item.generics, "a capability takes no generic parameters");
    }
    if !item.supertraits.is_empty() {
        errors.at(
            &item.supertraits,
            "a capability has no supertraits: its own methods are what it records",
        );
    }
    let mut methods = Vec::new();
    for entry in &mut item.items {
        match entry {
            TraitItem::Fn(function) => match strip_method(function, outside.is_some()) {
                Ok(Some(parsed)) => methods.push(parsed),
                Ok(None) => {}
                Err(error) => errors.push(error),
            },
            TraitItem::Type(ty) if ty.default.is_none() && ty.generics.params.is_empty() => {}
            other => errors.at(
                other,
                "a capability holds only methods and plain associated types",
            ),
        }
    }
    let mut recorded: Vec<Type> = Vec::new();
    let mut seen = Vec::new();
    for method in methods.iter().filter(|method| !method.redacted) {
        for ty in recorded_types(method) {
            if names_self(&ty) {
                errors.at(
                    &ty,
                    "a recorded type can't name `Self`; only a media method's error can",
                );
            } else if !seen.contains(&key(&ty)) {
                seen.push(key(&ty));
                recorded.push(ty);
            }
        }
    }
    let (recorder, replay) = match outside {
        Some(_) => (TokenStream::new(), TokenStream::new()),
        None => (
            recorder::expand(&item, id, &methods, &mut errors),
            replay::expand(&item, id, &methods),
        ),
    };
    let borrowed = recorder::borrowed(&item);
    errors.finish()?;

    let api = quote!(::device_api::capability);
    let name = item.ident.to_string();
    let table = format_ident!("CAPABILITY_{}", screaming(&name));
    let types = format_ident!("{}Recorded", item.ident);
    let vis = &item.vis;
    let world = match &outside {
        Some(reason) => quote!(#api::World::Outside(#reason)),
        None => quote!(#api::World::Recorded),
    };
    let rows = methods.iter().map(|method| row(&api, method));
    let list = recorded
        .iter()
        .rev()
        .fold(quote!(()), |rest, ty| quote!((#ty, #rest)));
    let enumerators = methods.iter().filter_map(|method| match &method.kind {
        Kind::Observe(Keys::All(keys)) => {
            let key = &method
                .args
                .iter()
                .find(|arg| arg.role == Role::Identity)?
                .ty;
            Some(quote!(const _: &[#key] = &#keys;))
        }
        _ => None,
    });
    let doc = format!("How [`{name}`] is recorded, method by method.");
    let types_doc =
        format!("Declared value types of [`{name}`] checked for trace encoding, as nested pairs.");
    Ok(quote! {
        #item
        #[doc = #doc]
        #vis const #table: #api::Capability = #api::Capability {
            id: #id,
            name: #name,
            world: #world,
            methods: &[#(#rows),*],
        };
        #[doc = #types_doc]
        #vis type #types = #list;
        #(#enumerators)*
        #borrowed
        #recorder
        #replay
    })
}

fn header(attr: TokenStream) -> syn::Result<(u8, Option<LitStr>)> {
    let (mut id, mut outside) = (None, None);
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("id") && id.is_none() {
            id = Some(meta.value()?.parse::<syn::LitInt>()?.base10_parse::<u8>()?);
            Ok(())
        } else if meta.path.is_ident("outside") && outside.is_none() {
            let reason = meta.value()?.parse::<LitStr>()?;
            if reason.value().trim().is_empty() {
                return Err(Error::new(reason.span(), "an outside capability says why"));
            }
            outside = Some(reason);
            Ok(())
        } else {
            Err(meta.error("expected `id = N` and, for a capability outside the recorded world, `outside = \"reason\"`"))
        }
    });
    syn::parse::Parser::parse2(parser, attr)?;
    let id = id.ok_or_else(|| {
        Error::new(
            proc_macro2::Span::call_site(),
            "a capability needs a stable `id = N`",
        )
    })?;
    Ok((id, outside))
}

fn strip_method(function: &mut TraitItemFn, outside: bool) -> syn::Result<Option<Method>> {
    let mut errors = Errors::default();
    if let Some(body) = &function.default {
        errors.at(
            body,
            "a capability method can't have a body: it would exist unrecorded",
        );
    }
    let sig = &mut function.sig;
    if !sig.generics.params.is_empty() {
        errors.at(
            &sig.generics,
            "a capability method takes no generic parameters; a callback is `impl FnMut(..)`",
        );
    }
    let (mut kind, mut marked) = (None, false);
    for attr in core::mem::take(&mut function.attrs) {
        if named(&attr, &CONDITIONAL) {
            errors.at(
                &attr,
                "a capability method exists in every build: under #[cfg] the table would list a method the trait lacks",
            );
            continue;
        }
        if named(&attr, &PASSTHROUGH) {
            function.attrs.push(attr);
            continue;
        }
        marked = true;
        match parse_kind(&attr) {
            Ok(_) if outside => errors.at(
                &attr,
                "an outside capability is not recorded, so its methods take no kind",
            ),
            Ok(_) if kind.is_some() => errors.at(&attr, "a method has one kind"),
            Ok(parsed) => kind = Some(parsed),
            Err(error) => errors.push(error),
        }
    }
    let receiver = match sig.receiver() {
        Some(receiver) if receiver.colon_token.is_some() => {
            errors.at(
                receiver,
                "write the receiver as `&self` or `&mut self`, not `self: Type`",
            );
            None
        }
        Some(receiver) if receiver.reference.is_some() => Some(receiver.mutability.is_some()),
        _ => {
            errors.at(&sig.ident, "a capability method borrows `self`");
            None
        }
    };
    let mut args = Vec::new();
    for input in sig.inputs.iter_mut() {
        let FnArg::Typed(arg) = input else { continue };
        let mut role = None;
        for attr in core::mem::take(&mut arg.attrs) {
            if named(&attr, &CONDITIONAL) {
                errors.at(
                    &attr,
                    "a capability argument exists in every build: #[cfg] would change the call the table describes",
                );
                continue;
            }
            if named(&attr, &PASSTHROUGH) {
                arg.attrs.push(attr);
                continue;
            }
            match parse_role(&attr) {
                Some(_) if role.is_some() => errors.at(&attr, "an argument has one role"),
                Some(parsed) => role = Some(parsed),
                None => errors.at(&attr, format!("unknown argument role: expected {ROLES}")),
            }
        }
        let role = match (role, &*arg.ty) {
            (None, Type::ImplTrait(bounds)) => {
                if !is_fn_mut(bounds) {
                    errors.at(
                        &arg.ty,
                        "an `impl` argument is a callback: `impl FnMut(..)`",
                    );
                }
                Role::Callback
            }
            (Some(_), Type::ImplTrait(_)) => {
                errors.at(&arg.ty, "a callback takes no role");
                Role::Callback
            }
            (Some(Role::Out), Type::Reference(r)) if r.mutability.is_some() => Role::Out,
            (Some(Role::Out), _) => {
                errors.at(&arg.ty, "#[out] marks a `&mut` buffer the device fills");
                Role::Out
            }
            (Some(Role::Input), Type::Reference(r)) if r.mutability.is_none() => Role::Input,
            (Some(Role::Input), _) => {
                errors.at(&arg.ty, "#[input] marks a `&` buffer VANA hands in");
                Role::Input
            }
            (_, Type::Reference(_)) => {
                errors.at(
                    &arg.ty,
                    "a borrowed argument is a buffer: mark it #[out] or #[input]",
                );
                Role::Identity
            }
            (role, _) => role.unwrap_or(Role::Identity),
        };
        match &*arg.pat {
            Pat::Ident(pat) => args.push(Arg {
                name: pat.ident.clone(),
                role,
                ty: (*arg.ty).clone(),
            }),
            other => errors.at(other, "name each argument"),
        }
    }
    let returns = match &sig.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, ty) => {
            if let Type::ImplTrait(_) = **ty {
                errors.at(
                    ty,
                    "a capability method returns a named type: an `impl Trait` can't be recorded or replayed",
                );
            }
            (**ty).clone()
        }
    };
    let Some(Declaration { kind, redacted }) = kind else {
        if !outside && !marked {
            let message = format!("`{}` has no kind: mark it {KINDS}", sig.ident);
            errors.at(&sig.ident, message);
        }
        return errors.finish().map(|()| None);
    };
    if let Some(mutable) = receiver {
        check(&kind, mutable, &args, &sig.ident, &mut errors);
    }
    errors.finish()?;
    Ok(Some(Method {
        name: sig.ident.clone(),
        kind,
        redacted,
        args,
        returns,
        sig: sig.clone(),
    }))
}

fn named(attr: &Attribute, names: &[&str]) -> bool {
    attr.path()
        .get_ident()
        .is_some_and(|name| names.iter().any(|kept| name == kept))
}

fn parse_kind(attr: &Attribute) -> syn::Result<Declaration> {
    let name = attr
        .path()
        .get_ident()
        .map(Ident::to_string)
        .unwrap_or_default();
    if name == "unrecorded" {
        let reason: LitStr = attr.parse_args().map_err(|_| {
            Error::new(
                attr.span(),
                "an unrecorded method says why: #[unrecorded(\"reason\")]",
            )
        })?;
        if reason.value().trim().is_empty() {
            return Err(Error::new(
                reason.span(),
                "an unrecorded method says why: the reason can't be empty",
            ));
        }
        return Ok(Declaration {
            kind: Kind::Unrecorded(reason),
            redacted: false,
        });
    }
    let allowed: &[&str] = match name.as_str() {
        "observe" => &["keyed", "keys", "redact"],
        "take" | "sink" => &["refreshes", "redact"],
        "request" => &["refreshes", "repeats", "redact"],
        "command" => &["redact"],
        "media" => &["read", "write", "erase"],
        "derived" => &[],
        _ => {
            return Err(Error::new(
                attr.span(),
                format!("unknown kind: expected {KINDS}"),
            ));
        }
    };
    let (mut given, mut keys) = (Vec::new(), None);
    match &attr.meta {
        syn::Meta::Path(_) => {}
        syn::Meta::List(list) if list.tokens.is_empty() => {
            return Err(Error::new(
                attr.span(),
                format!("write #[{name}] without empty parentheses"),
            ));
        }
        syn::Meta::List(_) => attr.parse_nested_meta(|meta| {
            let option = meta
                .path
                .get_ident()
                .map(Ident::to_string)
                .unwrap_or_default();
            if !allowed.contains(&option.as_str()) {
                let message = match allowed {
                    [] => format!("#[{name}] takes no options"),
                    [only] => format!("#[{name}] takes only `{only}`"),
                    [rest @ .., last] => {
                        format!("#[{name}] takes `{}` or `{last}`", rest.join("`, `"))
                    }
                };
                return Err(meta.error(message));
            }
            if given.contains(&option) {
                return Err(meta.error(format!("`{option}` is given twice")));
            }
            if option == "keys" {
                keys = Some(meta.value()?.parse::<Expr>()?);
            }
            given.push(option);
            Ok(())
        })?,
        syn::Meta::NameValue(_) => {
            return Err(Error::new(
                attr.span(),
                format!("#[{name}] takes no `= value`"),
            ));
        }
    }
    let has = |option: &str| given.iter().any(|given| given == option);
    let refreshes = has("refreshes");
    let kind = match name.as_str() {
        "observe" => Kind::Observe(match (has("keyed"), keys) {
            (false, None) => Keys::Unkeyed,
            (true, None) => Keys::Asked,
            (false, Some(keys)) => Keys::All(keys),
            (true, Some(_)) => {
                return Err(Error::new(
                    attr.span(),
                    "an observe is `keyed` or names its `keys`, not both",
                ));
            }
        }),
        "take" => Kind::Take { refreshes },
        "request" => {
            let repeats = has("repeats");
            if repeats && (!refreshes || has("redact")) {
                return Err(Error::new(
                    attr.span(),
                    "a request that `repeats` also `refreshes`, and has no secret to `redact`",
                ));
            }
            Kind::Request { refreshes, repeats }
        }
        "sink" => Kind::Sink { refreshes },
        "command" => Kind::Command,
        "derived" => Kind::Derived,
        _ => Kind::Media(match given.as_slice() {
            [one] if one == "read" => Media::Read,
            [one] if one == "write" => Media::Write,
            [one] if one == "erase" => Media::Erase,
            _ => {
                return Err(Error::new(
                    attr.span(),
                    "a media method does one thing: #[media(read)], #[media(write)] or #[media(erase)]",
                ));
            }
        }),
    };
    Ok(Declaration {
        kind,
        redacted: has("redact"),
    })
}

fn parse_role(attr: &Attribute) -> Option<Role> {
    attr.meta.require_path_only().ok()?;
    match attr.path().get_ident()?.to_string().as_str() {
        "clock" => Some(Role::Clock),
        "at" => Some(Role::At),
        "out" => Some(Role::Out),
        "input" => Some(Role::Input),
        "len" => Some(Role::Len),
        _ => None,
    }
}

fn is_fn_mut(bounds: &syn::TypeImplTrait) -> bool {
    bounds.bounds.iter().any(|bound| match bound {
        syn::TypeParamBound::Trait(t) => t.path.segments.last().is_some_and(|s| s.ident == "FnMut"),
        _ => false,
    })
}

fn check(kind: &Kind, mutable: bool, args: &[Arg], name: &Ident, errors: &mut Errors) {
    let reads = matches!(kind, Kind::Observe(_) | Kind::Derived);
    if reads && mutable {
        errors.at(
            name,
            "an observe or derived method only reads: it takes `&self`",
        );
    }
    if !reads && !mutable {
        errors.at(name, "this kind acts on the device: it takes `&mut self`");
    }
    let count = |role| args.iter().filter(|arg| arg.role == role).count();
    match kind {
        Kind::Observe(Keys::Unkeyed) if count(Role::Identity) > 0 => errors.at(
            name,
            "an observe is sampled before VANA asks, so its arguments are keys: declare #[observe(keys = ..)] or #[observe(keyed)]",
        ),
        Kind::Observe(Keys::Asked | Keys::All(_)) if count(Role::Identity) != 1 => {
            errors.at(name, "a keyed observe takes exactly one key argument")
        }
        Kind::Media(media) => {
            if count(Role::At) == 0 {
                errors.at(name, "a media method marks its address with #[at]");
            }
            let needs = match media {
                Media::Read => "#[out] buffer",
                Media::Write => "#[input] buffer",
                Media::Erase => "#[len]",
            };
            if count(media.role()) != 1 {
                errors.at(name, format!("this media method takes exactly one {needs}"));
            }
        }
        _ => {}
    }
    if count(Role::Callback) > 1 {
        errors.at(name, "a method takes one callback");
    }
    for arg in args {
        let allowed = match arg.role {
            Role::Identity => !matches!(kind, Kind::Media(_)),
            Role::Clock => true,
            Role::At => matches!(kind, Kind::Media(_)),
            Role::Len => matches!(kind, Kind::Media(Media::Erase)),
            Role::Out => matches!(kind, Kind::Take { .. } | Kind::Media(Media::Read)),
            Role::Input => matches!(kind, Kind::Sink { .. } | Kind::Media(Media::Write)),
            Role::Callback => matches!(kind, Kind::Sink { .. } | Kind::Unrecorded(_)),
        };
        if !allowed {
            let message = match arg.role {
                Role::Identity => {
                    "a media method's arguments each take a role: #[at], #[out], #[input] or #[len]"
                }
                Role::At => "#[at] marks a media address",
                Role::Len => "#[len] marks the length of a media erase",
                Role::Out => "#[out] belongs to a take or a media read",
                Role::Input => "#[input] belongs to a sink or a media write",
                _ => "only a sink or an unrecorded method takes a callback",
            };
            errors.at(&arg.name, message);
        }
    }
}

fn recorded_types(method: &Method) -> Vec<Type> {
    let identity = method
        .args
        .iter()
        .filter(|arg| arg.role == Role::Identity)
        .map(|arg| arg.ty.clone());
    match &method.kind {
        Kind::Derived | Kind::Unrecorded(_) => Vec::new(),
        Kind::Media(_) => method
            .args
            .iter()
            .filter(|arg| matches!(arg.role, Role::At | Role::Len))
            .map(|arg| arg.ty.clone())
            .collect(),
        Kind::Observe(_) => identity.chain([static_element(&method.returns)]).collect(),
        _ => identity.chain([method.returns.clone()]).collect(),
    }
}

fn static_element(ty: &Type) -> Type {
    match ty {
        Type::Reference(reference)
            if reference
                .lifetime
                .as_ref()
                .is_some_and(|l| l.ident == "static") =>
        {
            match &*reference.elem {
                Type::Slice(slice) => (*slice.elem).clone(),
                _ => ty.clone(),
            }
        }
        _ => ty.clone(),
    }
}

fn names_self(ty: &Type) -> bool {
    fn any(tokens: TokenStream) -> bool {
        tokens.into_iter().any(|token| match token {
            proc_macro2::TokenTree::Ident(ident) => ident == "Self",
            proc_macro2::TokenTree::Group(group) => any(group.stream()),
            _ => false,
        })
    }
    any(quote!(#ty))
}

fn key(ty: &Type) -> String {
    as_written(&quote!(#ty))
}

fn row(api: &TokenStream, method: &Method) -> TokenStream {
    let name = method.name.to_string();
    let kind = match &method.kind {
        Kind::Observe(Keys::Unkeyed) => quote!(#api::Kind::Observe(#api::Keys::Unkeyed)),
        Kind::Observe(Keys::Asked) => quote!(#api::Kind::Observe(#api::Keys::Asked)),
        Kind::Observe(Keys::All(keys)) => {
            let keys = as_written(&quote!(#keys));
            quote!(#api::Kind::Observe(#api::Keys::All(#keys)))
        }
        Kind::Take { refreshes } => quote!(#api::Kind::Take { refreshes: #refreshes }),
        Kind::Request { refreshes, repeats } => {
            quote!(#api::Kind::Request { refreshes: #refreshes, repeats: #repeats })
        }
        Kind::Sink { refreshes } => quote!(#api::Kind::Sink { refreshes: #refreshes }),
        Kind::Media(Media::Read) => quote!(#api::Kind::Media(#api::Media::Read)),
        Kind::Media(Media::Write) => quote!(#api::Kind::Media(#api::Media::Write)),
        Kind::Media(Media::Erase) => quote!(#api::Kind::Media(#api::Media::Erase)),
        Kind::Command => quote!(#api::Kind::Command),
        Kind::Derived => quote!(#api::Kind::Derived),
        Kind::Unrecorded(reason) => quote!(#api::Kind::Unrecorded(#reason)),
    };
    let args = method.args.iter().map(|arg| {
        let name = arg.name.to_string();
        let ty = key(&arg.ty);
        let role = format_ident!(
            "{}",
            match arg.role {
                Role::Identity => "Identity",
                Role::Clock => "Clock",
                Role::At => "At",
                Role::Out => "Out",
                Role::Input => "Input",
                Role::Len => "Len",
                Role::Callback => "Callback",
            }
        );
        quote!(#api::Arg { name: #name, role: #api::Role::#role, ty: #ty })
    });
    let (returns, redacted) = (key(&method.returns), method.redacted);
    quote!(#api::Method {
        name: #name,
        kind: #kind,
        redacted: #redacted,
        args: &[#(#args),*],
        returns: #returns,
    })
}

fn screaming(name: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if c.is_uppercase() && previous_lower {
            out.push('_');
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        out.extend(c.to_uppercase());
    }
    out
}

fn as_written(tokens: &TokenStream) -> String {
    let spaced: Vec<char> = tokens.to_string().chars().collect();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::new();
    for (index, &c) in spaced.iter().enumerate() {
        let before = out.chars().last();
        let after = spaced.get(index + 1).copied();
        if c != ' '
            || matches!(before, Some(',' | ';'))
            || word(before) && (word(after) || after == Some('['))
        {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_print_as_written() {
        let printed: Vec<String> = [
            quote!(Option<(u8, u16, u64)>),
            quote!(&'static [Pattern]),
            quote!(&mut [u8; SECTOR_SIZE]),
            quote!(impl FnMut(usize, &mut [u16])),
            quote!([Option<crate::ant::Snapshot>; crate::ant::CHANNEL_CAPACITY]),
            quote!(Result<(), Self::Error>),
        ]
        .iter()
        .map(as_written)
        .collect();
        assert_eq!(
            printed,
            [
                "Option<(u8, u16, u64)>",
                "&'static [Pattern]",
                "&mut [u8; SECTOR_SIZE]",
                "impl FnMut(usize, &mut [u16])",
                "[Option<crate::ant::Snapshot>; crate::ant::CHANNEL_CAPACITY]",
                "Result<(), Self::Error>",
            ]
        );
    }

    #[test]
    fn table_names_are_the_trait_in_capitals() {
        let names: Vec<String> = ["Ant", "ReadWrite", "OwnedFlash", "InputObservation"]
            .iter()
            .map(|name| screaming(name))
            .collect();
        assert_eq!(
            names,
            ["ANT", "READ_WRITE", "OWNED_FLASH", "INPUT_OBSERVATION"]
        );
    }
}
