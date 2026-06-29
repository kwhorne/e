//! Framework-aware completion: Flux UI components, Livewire `wire:` directives,
//! Tailwind utility classes, and Vue/Svelte directives. Returns the number of
//! bytes before the caret to replace, plus the items (with full `insert_text`),
//! so multi-segment tokens like `flux:button` or `bg-blue-500` are replaced
//! correctly even though they contain non-word characters.

use lsp_types::{CompletionItem, CompletionItemKind};

use e_core::language::Language;

fn ci(label: String, insert: String, detail: &str) -> CompletionItem {
    CompletionItem {
        label,
        insert_text: Some(insert),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(detail.to_string()),
        ..Default::default()
    }
}

/// The trailing run of `line` whose characters all satisfy `allowed`.
fn token(line: &str, allowed: impl Fn(char) -> bool) -> &str {
    let mut start = line.len();
    for (i, c) in line.char_indices().rev() {
        if allowed(c) {
            start = i;
        } else {
            break;
        }
    }
    &line[start..]
}

const FLUX: &[&str] = &[
    "button",
    "modal",
    "modal.trigger",
    "modal.close",
    "heading",
    "subheading",
    "text",
    "field",
    "input",
    "input.group",
    "label",
    "description",
    "error",
    "textarea",
    "select",
    "select.option",
    "checkbox",
    "checkbox.group",
    "radio",
    "radio.group",
    "switch",
    "badge",
    "avatar",
    "card",
    "callout",
    "callout.heading",
    "callout.text",
    "separator",
    "icon",
    "dropdown",
    "menu",
    "menu.item",
    "menu.separator",
    "menu.submenu",
    "navbar",
    "navbar.item",
    "navlist",
    "navlist.item",
    "navlist.group",
    "navmenu",
    "breadcrumbs",
    "breadcrumbs.item",
    "tabs",
    "tab",
    "tab.panel",
    "table",
    "table.columns",
    "table.column",
    "table.rows",
    "table.row",
    "table.cell",
    "pagination",
    "tooltip",
    "profile",
    "brand",
    "spacer",
    "link",
    "command",
    "command.input",
    "command.items",
    "command.item",
    "date-picker",
    "calendar",
    "editor",
    "toast",
    "accordion",
    "accordion.item",
    "accordion.heading",
    "accordion.content",
    "autocomplete",
    "header",
    "sidebar",
    "main",
    "container",
    "dropdown.button",
    "dropdown.menu",
];

const WIRE: &[&str] = &[
    "wire:model",
    "wire:model.live",
    "wire:model.live.debounce.500ms",
    "wire:model.blur",
    "wire:model.lazy",
    "wire:model.defer",
    "wire:model.number",
    "wire:click",
    "wire:click.prevent",
    "wire:submit",
    "wire:submit.prevent",
    "wire:change",
    "wire:keydown",
    "wire:keydown.enter",
    "wire:keyup",
    "wire:loading",
    "wire:loading.remove",
    "wire:loading.class",
    "wire:loading.attr",
    "wire:loading.delay",
    "wire:target",
    "wire:poll",
    "wire:poll.visible",
    "wire:poll.keep-alive",
    "wire:init",
    "wire:key",
    "wire:ignore",
    "wire:ignore.self",
    "wire:navigate",
    "wire:navigate.hover",
    "wire:dirty",
    "wire:dirty.class",
    "wire:offline",
    "wire:confirm",
    "wire:current",
    "wire:replace",
    "wire:transition",
    "wire:show",
    "wire:stream",
    "wire:cloak",
];

const VUE: &[&str] = &[
    "v-if",
    "v-else",
    "v-else-if",
    "v-for",
    "v-show",
    "v-model",
    "v-bind",
    "v-on",
    "v-html",
    "v-text",
    "v-slot",
    "v-pre",
    "v-once",
    "v-memo",
    "v-cloak",
    "@click",
    "@submit",
    "@submit.prevent",
    "@input",
    "@change",
    "@keydown",
    "@keyup.enter",
    "@focus",
    "@blur",
    ":class",
    ":style",
    ":key",
    ":value",
    ":disabled",
    ":href",
    ":src",
];

const SVELTE: &[&str] = &[
    "on:click",
    "on:submit",
    "on:submit|preventDefault",
    "on:input",
    "on:change",
    "on:keydown",
    "on:focus",
    "on:blur",
    "bind:value",
    "bind:this",
    "bind:checked",
    "bind:group",
    "bind:files",
    "use:action",
    "transition:fade",
    "transition:fly",
    "transition:slide",
    "in:fade",
    "out:fade",
    "animate:flip",
    "class:active",
];

const TAILWIND: &[&str] = &[
    // layout / display
    "container",
    "block",
    "inline-block",
    "inline",
    "flex",
    "inline-flex",
    "grid",
    "hidden",
    "table",
    "contents",
    "flow-root",
    // flexbox / grid
    "flex-row",
    "flex-col",
    "flex-wrap",
    "flex-nowrap",
    "flex-1",
    "flex-auto",
    "flex-none",
    "grow",
    "shrink",
    "items-start",
    "items-center",
    "items-end",
    "items-stretch",
    "items-baseline",
    "justify-start",
    "justify-center",
    "justify-end",
    "justify-between",
    "justify-around",
    "justify-evenly",
    "content-center",
    "self-center",
    "self-start",
    "grid-cols-1",
    "grid-cols-2",
    "grid-cols-3",
    "grid-cols-4",
    "grid-cols-6",
    "grid-cols-12",
    "col-span-1",
    "col-span-2",
    "col-span-3",
    "col-span-full",
    "gap-0",
    "gap-1",
    "gap-2",
    "gap-3",
    "gap-4",
    "gap-6",
    "gap-8",
    "space-x-2",
    "space-x-4",
    "space-y-2",
    "space-y-4",
    // spacing
    "p-0",
    "p-1",
    "p-2",
    "p-3",
    "p-4",
    "p-5",
    "p-6",
    "p-8",
    "p-10",
    "p-12",
    "px-2",
    "px-3",
    "px-4",
    "px-6",
    "px-8",
    "py-1",
    "py-2",
    "py-3",
    "py-4",
    "py-6",
    "pt-2",
    "pt-4",
    "pb-2",
    "pb-4",
    "pl-2",
    "pr-2",
    "m-0",
    "m-1",
    "m-2",
    "m-4",
    "m-auto",
    "mx-auto",
    "mx-2",
    "mx-4",
    "my-2",
    "my-4",
    "mt-1",
    "mt-2",
    "mt-4",
    "mt-6",
    "mt-8",
    "mb-2",
    "mb-4",
    "mb-6",
    "ml-2",
    "mr-2",
    // sizing
    "w-full",
    "w-screen",
    "w-auto",
    "w-fit",
    "w-1/2",
    "w-1/3",
    "w-2/3",
    "w-1/4",
    "w-px",
    "w-4",
    "w-6",
    "w-8",
    "w-10",
    "w-12",
    "w-16",
    "w-20",
    "w-24",
    "w-32",
    "w-64",
    "h-full",
    "h-screen",
    "h-auto",
    "h-fit",
    "h-4",
    "h-6",
    "h-8",
    "h-10",
    "h-12",
    "h-16",
    "h-20",
    "h-24",
    "h-32",
    "min-h-screen",
    "min-h-full",
    "min-w-0",
    "max-w-xs",
    "max-w-sm",
    "max-w-md",
    "max-w-lg",
    "max-w-xl",
    "max-w-2xl",
    "max-w-4xl",
    "max-w-full",
    "size-4",
    "size-5",
    "size-6",
    "size-8",
    // typography
    "text-xs",
    "text-sm",
    "text-base",
    "text-lg",
    "text-xl",
    "text-2xl",
    "text-3xl",
    "text-4xl",
    "font-thin",
    "font-light",
    "font-normal",
    "font-medium",
    "font-semibold",
    "font-bold",
    "font-extrabold",
    "italic",
    "underline",
    "uppercase",
    "lowercase",
    "capitalize",
    "text-left",
    "text-center",
    "text-right",
    "leading-none",
    "leading-tight",
    "leading-normal",
    "leading-relaxed",
    "tracking-tight",
    "tracking-wide",
    "truncate",
    "whitespace-nowrap",
    // colors
    "bg-transparent",
    "bg-white",
    "bg-black",
    "bg-gray-50",
    "bg-gray-100",
    "bg-gray-200",
    "bg-gray-800",
    "bg-gray-900",
    "bg-red-500",
    "bg-green-500",
    "bg-blue-500",
    "bg-blue-600",
    "bg-indigo-600",
    "bg-zinc-900",
    "text-white",
    "text-black",
    "text-gray-400",
    "text-gray-500",
    "text-gray-600",
    "text-gray-700",
    "text-gray-900",
    "text-red-500",
    "text-green-600",
    "text-blue-600",
    "border-gray-200",
    "border-gray-300",
    "border-zinc-700",
    // borders / radius / effects
    "border",
    "border-0",
    "border-2",
    "border-t",
    "border-b",
    "border-l",
    "border-r",
    "rounded",
    "rounded-sm",
    "rounded-md",
    "rounded-lg",
    "rounded-xl",
    "rounded-2xl",
    "rounded-full",
    "rounded-none",
    "shadow",
    "shadow-sm",
    "shadow-md",
    "shadow-lg",
    "shadow-xl",
    "shadow-none",
    "ring",
    "ring-1",
    "ring-2",
    "opacity-0",
    "opacity-50",
    "opacity-75",
    "opacity-100",
    // position
    "relative",
    "absolute",
    "fixed",
    "sticky",
    "static",
    "inset-0",
    "top-0",
    "bottom-0",
    "left-0",
    "right-0",
    "z-0",
    "z-10",
    "z-20",
    "z-50",
    "overflow-hidden",
    "overflow-auto",
    "overflow-y-auto",
    // interactivity
    "cursor-pointer",
    "cursor-default",
    "select-none",
    "pointer-events-none",
    "transition",
    "transition-all",
    "duration-150",
    "duration-200",
    "ease-in-out",
    // common variants users start typing
    "hover:",
    "focus:",
    "active:",
    "disabled:",
    "group-hover:",
    "dark:",
    "sm:",
    "md:",
    "lg:",
    "xl:",
];

/// Component/attribute completions for a `namespace:rest` style token.
fn namespaced(
    line: &str,
    ns: &str,
    names: &[&str],
    detail: &str,
    needs_lt: bool,
) -> Option<(usize, Vec<CompletionItem>)> {
    let tok = token(line, |c| {
        c.is_ascii_alphanumeric() || c == '-' || c == ':' || c == '.' || c == '|'
    });
    if tok.is_empty() {
        return None;
    }
    let before = &line[..line.len() - tok.len()];
    if needs_lt && !before.ends_with('<') {
        return None;
    }
    let head = tok.split(':').next().unwrap_or("");
    let with_colon = tok.contains(':');
    let ok = if with_colon {
        head == ns
    } else {
        !head.is_empty() && ns.starts_with(head)
    };
    if !ok {
        return None;
    }
    let mut items = Vec::new();
    for &name in names {
        // `name` is either a full attribute (e.g. "wire:click") or a bare
        // component (e.g. "button"). Build the full inserted token.
        let full = if name.contains(':') {
            name.to_string()
        } else {
            format!("{ns}:{name}")
        };
        if full.starts_with(tok)
            || (with_colon && full.starts_with(&format!("{ns}:")) && {
                let rest = &full[ns.len() + 1..];
                let typed = tok.split_once(':').map(|x| x.1).unwrap_or("");
                rest.starts_with(typed)
            })
        {
            items.push(ci(full.clone(), full, detail));
        }
    }
    if items.is_empty() {
        None
    } else {
        Some((tok.len(), items))
    }
}

fn directives(line: &str, names: &[&str], detail: &str) -> Option<(usize, Vec<CompletionItem>)> {
    let tok = token(line, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '.' | '@' | '|')
    });
    if tok.is_empty() {
        return None;
    }
    let items: Vec<CompletionItem> = names
        .iter()
        .filter(|n| n.starts_with(tok))
        .map(|n| ci(n.to_string(), n.to_string(), detail))
        .collect();
    if items.is_empty() {
        None
    } else {
        Some((tok.len(), items))
    }
}

fn tailwind(line: &str) -> Option<(usize, Vec<CompletionItem>)> {
    // Must be inside a class / :class / class:list attribute value.
    let lt = line.rfind('<')?;
    let tag = &line[lt..];
    let cpos = tag
        .rfind("class=")
        .or_else(|| tag.rfind(":class="))
        .or_else(|| tag.rfind("className="))?;
    let after = &tag[cpos..];
    // The attribute's quote must still be open.
    let quote = after.find(['"', '\''])?;
    let inside = &after[quote + 1..];
    if inside.contains(after.as_bytes()[quote] as char) {
        return None; // closing quote already present before the caret
    }
    let tok = token(line, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '/' | '.' | '[' | ']' | '%' | '#')
    });
    if tok.is_empty() {
        return None;
    }
    let items: Vec<CompletionItem> = TAILWIND
        .iter()
        .filter(|c| c.starts_with(tok))
        .take(60)
        .map(|c| ci(c.to_string(), c.to_string(), "Tailwind"))
        .collect();
    if items.is_empty() {
        None
    } else {
        Some((tok.len(), items))
    }
}

/// Try every framework provider relevant to `language`. Returns
/// `(replace_len, items)` when one matches.
pub fn completions(language: Language, line_before: &str) -> Option<(usize, Vec<CompletionItem>)> {
    use Language::*;
    match language {
        Blade | Html | Php => {
            // Flux component tags, then Livewire attributes, then Tailwind.
            namespaced(line_before, "flux", FLUX, "Flux UI", true)
                .or_else(|| namespaced(line_before, "wire", WIRE, "Livewire", false))
                .or_else(|| tailwind(line_before))
        }
        Vue => directives(line_before, VUE, "Vue").or_else(|| tailwind(line_before)),
        Svelte => directives(line_before, SVELTE, "Svelte").or_else(|| tailwind(line_before)),
        Css => None,
        _ => None,
    }
}
