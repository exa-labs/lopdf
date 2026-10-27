use lopdf::{Document, LazyDocument, LoadOptions, Object, SaveOptions};

/// The lazy reader must see the document the eager loader sees: the same pages in the
/// same order, and the same page and annotation objects.
fn assert_matches_eager(buffer: &[u8], name: &str) {
    let eager = Document::load_mem(buffer).unwrap();
    let lazy = LazyDocument::load_mem(buffer, LoadOptions::default()).unwrap();

    let eager_pages: Vec<_> = eager.get_pages().into_values().collect();
    assert_eq!(lazy.page_ids().unwrap(), eager_pages, "{name}: page order");

    for page_id in eager_pages {
        let page = lazy.get_object(page_id).unwrap();
        assert_eq!(&page, eager.get_object(page_id).unwrap(), "{name}: page {page_id:?}");
        let Ok(annots) = page.as_dict().unwrap().get(b"Annots") else {
            continue;
        };
        let Object::Array(annots) = lazy.resolve(annots).unwrap() else {
            panic!("{name}: /Annots of page {page_id:?} is not an array");
        };
        for annot in &annots {
            let id = annot.as_reference().unwrap();
            assert_eq!(
                &lazy.get_object(id).unwrap(),
                eager.get_object(id).unwrap(),
                "{name}: annotation {id:?}"
            );
        }
    }
}

#[test]
fn lazy_document_matches_eager_load() {
    for path in [
        "assets/example.pdf",
        "assets/AnnotationDemo.pdf",
        "assets/Incremental.pdf",
        "assets/unicode.pdf",
    ] {
        assert_matches_eager(&std::fs::read(path).unwrap(), path);
    }
}

/// Objects stored in object streams resolve through the cross-reference stream.
#[test]
fn lazy_document_reads_object_streams() {
    let mut doc = Document::load_mem(&std::fs::read("assets/AnnotationDemo.pdf").unwrap()).unwrap();
    let mut buffer = Vec::new();
    let options = SaveOptions::builder()
        .use_object_streams(true)
        .use_xref_streams(true)
        .build();
    doc.save_with_options(&mut buffer, options).unwrap();

    let resaved = Document::load_mem(&buffer).unwrap();
    let compressed = resaved
        .reference_table
        .entries
        .values()
        .filter(|entry| matches!(entry, lopdf::xref::XrefEntry::Compressed { .. }))
        .count();
    assert!(
        compressed > 0,
        "the re-saved document stores no objects in object streams"
    );

    let lazy = LazyDocument::load_mem(&buffer, LoadOptions::default()).unwrap();
    let page = lazy.page_ids().unwrap()[0];
    assert!(lazy.trailer().get(b"Root").is_ok());
    assert!(lazy.get_object(page).unwrap().as_dict().unwrap().has(b"Annots"));
    assert_matches_eager(&buffer, "object streams");
}

/// An encrypted document opens with the empty password and its objects come back decrypted.
#[test]
fn lazy_document_decrypts_objects() {
    assert_matches_eager(&std::fs::read("assets/encrypted.pdf").unwrap(), "assets/encrypted.pdf");
}

#[test]
fn lazy_document_resolve_rejects_reference_cycles() {
    let mut doc = Document::with_version("1.5");
    let a = doc.new_object_id();
    let b = doc.new_object_id();
    doc.objects.insert(a, Object::Reference(b));
    doc.objects.insert(b, Object::Reference(a));
    let pages = doc.add_object(lopdf::dictionary! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 });
    let catalog = doc.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);
    let mut buffer = Vec::new();
    doc.save_to(&mut buffer).unwrap();

    let lazy = LazyDocument::load_mem(&buffer, LoadOptions::default()).unwrap();
    assert!(lazy.resolve(&Object::Reference(a)).is_err());
    assert_eq!(lazy.page_ids().unwrap(), Vec::new());
}
