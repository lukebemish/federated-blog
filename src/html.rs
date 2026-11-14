use lol_html::{RewriteStrSettings, doc_comments, doctype, element, html_content::ContentType, rewrite_str};

use crate::Error;

pub(crate) fn strip_tags(html: &String) -> Result<String, Error> {
    let block_tags = vec![
        "p", "div", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6", 
        "tr", "blockquote", "pre", "section", "article", "header", "footer"
    ].join(", ");
    let ignore_tags = vec!["script", "style", "head"].join(", ");

    let rewriter = RewriteStrSettings::new().append_element_content_handler(
        element!(ignore_tags, |el| {
            el.remove();
            Ok(())
        })
    ).append_element_content_handler(
        element!(block_tags, |el| {
            el.append("\n", ContentType::Text);
            el.remove_and_keep_content();
            Ok(())
        })
    ).append_element_content_handler(
        element!("*", |el| {
            el.remove_and_keep_content();
            Ok(())
        }),
    ).append_document_content_handler(
        doc_comments!(|c| {
            c.remove();
            Ok(())
        })
    ).append_document_content_handler(
        doctype!(|d| {
            d.remove();
            Ok(())
        })
    );
    let rewritten = rewrite_str(html, rewriter)?;
    let rewritten = rewritten.split('\n')
        .map(|line| {
            line.trim()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(rewritten)
}
