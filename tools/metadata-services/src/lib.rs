//! Application-side metadata services. The codec does not depend on this crate.
//! This runnable example wires xmpkit, bounded decompression, and exact known ICC
//! normalization through zencodecs' runtime Services interface.
use enough::Stop;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use xmpkit::XmpMeta;
use zencodecs::metadata::{Compression, Error, Services};
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";

/// Bounded XML preflight before giving the packet to an RDF editor.
/// Xmpkit 0.1.6 is not a hardened XML boundary: bound bytes/nodes/depth,
/// reject DTDs, alternate subjects, duplicate properties and prefix rebinding.
/// Inspection can still show unsupported packets using zencodec::metadata_audit.
fn preflight(xml: &str, limit: usize) -> Result<(), Error> {
    if xml.len() > limit {
        return Err(Error::Limit("XMP bytes"));
    }
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 65_536,
            ..Default::default()
        },
    )
    .map_err(|_| Error::Malformed("XMP XML"))?;
    let mut bindings = BTreeMap::new();
    let mut properties = BTreeSet::new();
    let descriptions: Vec<_> = doc
        .descendants()
        .filter(|n| {
            n.has_tag_name((RDF, "Description"))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .collect();
    if descriptions.is_empty() {
        return Err(Error::Unsupported(
            "XMP requires an RDF subject for editing",
        ));
    }
    for n in doc.descendants().filter(|n| n.is_element()) {
        if n.ancestors().take(66).count() > 65 {
            return Err(Error::Limit("XMP depth"));
        }
        for ns in n.namespaces() {
            if let Some(old) = bindings.insert(ns.name(), ns.uri())
                && old != ns.uri()
            {
                return Err(Error::Unsupported("XMP prefix rebinding"));
            }
        }
    }
    for d in descriptions {
        if d.attribute((RDF, "about")).is_some_and(|s| !s.is_empty()) {
            return Err(Error::Unsupported("XMP alternate subject"));
        }
        for a in d.attributes() {
            if a.namespace() == Some(RDF) {
                if a.name() != "about" {
                    return Err(Error::Unsupported("XMP RDF subject qualifier"));
                }
                continue;
            }
            if !properties.insert((a.namespace(), a.name())) {
                return Err(Error::Malformed("duplicate XMP property"));
            }
        }
        for n in d.children().filter(|n| n.is_element()) {
            if !properties.insert((n.tag_name().namespace(), n.tag_name().name())) {
                return Err(Error::Malformed("duplicate XMP property"));
            }
        }
    }
    Ok(())
}

/// Explicit arbitrary edit of the supported packet model, serialized by xmpkit.
/// Structural audit/diff remains available separately; this is not arbitrary RDF
/// graph canonicalization or a byte-preserving serializer.
pub fn edit_xmp(
    xml: &str,
    limit: usize,
    edit: impl FnOnce(&mut XmpMeta) -> Result<(), Error>,
) -> Result<String, Error> {
    preflight(xml, limit)?;
    let mut meta = XmpMeta::parse(xml).map_err(|_| Error::Service("xmpkit parse"))?;
    edit(&mut meta)?;
    let serialized = meta
        .serialize_packet()
        .map_err(|_| Error::Service("xmpkit serialize"))?;
    preflight(&serialized, limit)?;
    Ok(serialized)
}

/// Example application policy: retain only XMP rights/creator fields when asked.
/// The library's minimal scrub does not need an XML editor at all.
#[derive(Default)]
pub struct PublishingServices {
    pub keep_rights: bool,
}
impl Services for PublishingServices {
    fn merge_xmp(
        &self,
        descriptive: &[u8],
        required: &[u8],
        limit: usize,
        stop: &dyn Stop,
    ) -> Result<Vec<u8>, Error> {
        merge_packets(descriptive, required, limit, stop)
    }
    fn rewrite_xmp(
        &self,
        xml: &[u8],
        limit: usize,
        stop: &dyn Stop,
    ) -> Result<Option<Vec<u8>>, Error> {
        stop.check().map_err(|_| Error::Cancelled)?;
        if !self.keep_rights {
            return Ok(None);
        }
        let xml = std::str::from_utf8(xml).map_err(|_| Error::Malformed("XMP UTF-8"))?;
        let output = edit_xmp(xml, limit, |meta| {
            for property in meta.all_properties() {
                let keep = (property.namespace_uri == DC
                    && matches!(property.name.as_str(), "creator" | "rights"))
                    || (property.namespace_uri == RIGHTS
                        && matches!(
                            property.name.as_str(),
                            "Marked" | "UsageTerms" | "WebStatement"
                        ));
                if !keep {
                    meta.delete_property(&property.namespace_uri, &property.name)
                        .map_err(|_| Error::Service("XMP delete"))?;
                }
            }
            meta.set_about_uri("");
            Ok(())
        })?;
        // Qualifiers/nested resources on retained rights fields can themselves
        // carry private data. This example allows only plain strings, arrays,
        // xml:lang and RDF array structure; richer structures require a policy.
        let doc =
            roxmltree::Document::parse(&output).map_err(|_| Error::Service("serialized XMP"))?;
        for d in doc
            .descendants()
            .filter(|n| n.has_tag_name((RDF, "Description")))
        {
            for a in d.attributes() {
                let ns = a.namespace();
                let name = a.name();
                let allowed = (ns == Some(RDF) && name == "about" && a.value().is_empty())
                    || (ns == Some(DC) && matches!(name, "creator" | "rights"))
                    || (ns == Some(RIGHTS)
                        && matches!(name, "Marked" | "UsageTerms" | "WebStatement"));
                if !allowed {
                    return Err(Error::Unsupported(
                        "toolkit could not remove an unknown XMP attribute",
                    ));
                }
            }

            for property in d.children().filter(|n| n.is_element()) {
                let ns = property.tag_name().namespace();
                let name = property.tag_name().name();
                if !((ns == Some(DC) && matches!(name, "creator" | "rights"))
                    || (ns == Some(RIGHTS)
                        && matches!(name, "Marked" | "UsageTerms" | "WebStatement")))
                {
                    return Err(Error::Unsupported(
                        "toolkit could not remove an unknown XMP property",
                    ));
                }

                for n in property.descendants().filter(|n| n.is_element()) {
                    if n != property
                        && !(n.tag_name().namespace() == Some(RDF)
                            && matches!(n.tag_name().name(), "Seq" | "Bag" | "Alt" | "li"))
                    {
                        return Err(Error::Unsupported(
                            "structured rights require an explicit application policy",
                        ));
                    }
                    if n.attributes().any(|a| {
                        !(a.namespace() == Some("http://www.w3.org/XML/1998/namespace")
                            && a.name() == "lang")
                    }) {
                        return Err(Error::Unsupported(
                            "qualified rights require an explicit application policy",
                        ));
                    }
                }
            }
        }
        stop.check().map_err(|_| Error::Cancelled)?;
        Ok(Some(output.into_bytes()))
    }
    fn normalize_icc(
        &self,
        icc: &[u8],
        limit: usize,
        stop: &dyn Stop,
    ) -> Result<Option<Vec<u8>>, Error> {
        stop.check().map_err(|_| Error::Cancelled)?;
        if icc.len() > limit {
            return Err(Error::Limit("ICC bytes"));
        }
        Ok(zenpixels_convert::icc_profiles::normalize_known_icc(icc)
            .filter(|&canonical| canonical != icc)
            .map(<[u8]>::to_vec))
    }
    fn decompress(
        &self,
        kind: Compression,
        data: &[u8],
        limit: usize,
        stop: &dyn Stop,
    ) -> Result<Vec<u8>, Error> {
        match kind {
            Compression::Zlib => inflate_zlib(data, limit, stop),
            Compression::Brotli => {
                check_brotli_window(data, limit)?;
                read_bounded(brotli::Decompressor::new(data, 4096), limit, stop)
            }
            _ => Err(Error::Unsupported("metadata compression kind")),
        }
    }
    fn compress(
        &self,
        kind: Compression,
        data: &[u8],
        limit: usize,
        stop: &dyn Stop,
    ) -> Result<Vec<u8>, Error> {
        if data.len() > limit {
            return Err(Error::Limit("metadata compression input"));
        }
        stop.check().map_err(|_| Error::Cancelled)?;
        // PNG ICC is the only compression producer in the current rewrite API.
        if kind != Compression::Zlib {
            return Err(Error::Unsupported("example Brotli encoder"));
        }
        let writer = BoundedWriter {
            bytes: Vec::new(),
            limit,
            stop,
        };
        let mut encoder = flate2::write::ZlibEncoder::new(writer, flate2::Compression::fast());
        encoder
            .write_all(data)
            .map_err(|_| Error::Service("bounded metadata compression"))?;
        Ok(encoder
            .finish()
            .map_err(|_| Error::Service("metadata compression"))?
            .bytes)
    }
}
fn read_bounded(mut reader: impl Read, limit: usize, stop: &dyn Stop) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut buf = [0; 4096];
    loop {
        stop.check().map_err(|_| Error::Cancelled)?;
        // Read one byte past the remaining budget at most, detecting excess
        // without materializing attacker-controlled decompressed output first.
        let capacity = (limit - out.len()).saturating_add(1).min(buf.len());
        let n = reader
            .read(&mut buf[..capacity])
            .map_err(|_| Error::Malformed("compressed metadata"))?;
        if n == 0 {
            return Ok(out);
        }
        if n > limit - out.len() {
            return Err(Error::Limit("expanded metadata"));
        }
        out.try_reserve(n).map_err(|_| Error::Allocation)?;
        out.extend_from_slice(&buf[..n]);
    }
}
struct BoundedWriter<'a> {
    bytes: Vec<u8>,
    limit: usize,
    stop: &'a dyn Stop,
}
impl Write for BoundedWriter<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.stop
            .check()
            .map_err(|_| std::io::Error::other("cancelled"))?;
        if b.len() > self.limit - self.bytes.len() {
            return Err(std::io::Error::other("metadata limit"));
        }
        self.bytes
            .try_reserve(b.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) const PACKET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:p="http://example.org/private/" p:Device="SECRET"><dc:creator><rdf:Seq><rdf:li>A &amp; B</rdf:li><rdf:li>C</rdf:li></rdf:Seq></dc:creator><dc:rights><rdf:Alt><rdf:li xml:lang="x-default">Copyright &lt;owner&gt;</rdf:li><rdf:li xml:lang="fr">Droits réservés</rdf:li></rdf:Alt></dc:rights><p:Nested rdf:parseType="Resource"><p:Name>unknown structure</p:Name></p:Nested></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    #[test]
    fn xmpkit_preserves_arrays_language_qualifiers_unknown_structures_and_escapes() {
        let output = edit_xmp(PACKET, 1 << 20, |m| {
            m.delete_property("http://example.org/private/", "Device")
                .map_err(|_| Error::Service("delete"))
        })
        .unwrap();
        assert!(!output.contains("SECRET"));
        let a = XmpMeta::parse(PACKET).unwrap();
        let b = XmpMeta::parse(&output).unwrap();
        for (ns, name) in [
            (DC, "creator"),
            (DC, "rights"),
            ("http://example.org/private/", "Nested"),
        ] {
            assert_eq!(a.get_property(ns, name), b.get_property(ns, name), "{name}");
        }
        let xml = roxmltree::Document::parse(&output).unwrap();
        assert!(xml.descendants().any(|n| {
            n.attribute(("http://www.w3.org/XML/1998/namespace", "lang")) == Some("fr")
                && n.text() == Some("Droits réservés")
        }));
        assert!(output.contains("&amp;"));
        assert!(output.contains("&lt;"));
    }
    #[test]
    fn publishing_policy_removes_unknown_properties_but_keeps_rights_and_languages() {
        let services = PublishingServices { keep_rights: true };
        let out = services
            .rewrite_xmp(PACKET.as_bytes(), 1 << 20, &enough::Unstoppable)
            .unwrap()
            .unwrap();
        let text = std::str::from_utf8(&out).unwrap();
        assert!(!text.contains("SECRET"));
        assert!(!text.contains("unknown structure"));
        let meta = XmpMeta::parse(text).unwrap();
        assert!(meta.get_property(DC, "creator").is_some());
        assert!(meta.get_property(DC, "rights").is_some());
        assert!(text.contains("xml:lang=\"fr\""));
    }
    #[test]
    fn invalid_duplicate_rebound_and_over_budget_xml_refuse_before_edit() {
        for bad in [
            "<!DOCTYPE x [<!ENTITY x 'PRIVATE'>]><x>&x;</x>",
            "<x>",
            "<x/>",
            &PACKET.replace("<dc:creator>", "<dc:creator>one</dc:creator><dc:creator>"),
            &PACKET.replace("<p:Name>", "<p:Name xmlns:p=\"http://other.example/\">"),
        ] {
            assert!(edit_xmp(bad, 1 << 20, |_| panic!("must not invoke editor")).is_err());
        }
        assert!(edit_xmp(PACKET, 4, |_| panic!("must not invoke editor")).is_err());
    }
    #[test]
    fn decompression_limits_are_enforced_during_expansion() {
        let service = PublishingServices::default();
        let plain = vec![b'A'; 100_000];
        let compressed = service
            .compress(Compression::Zlib, &plain, 200_000, &enough::Unstoppable)
            .unwrap();
        assert!(compressed.len() < 1000);
        assert!(matches!(
            service.decompress(Compression::Zlib, &compressed, 1024, &enough::Unstoppable),
            Err(Error::Limit(_))
        ));
        assert_eq!(
            service
                .decompress(
                    Compression::Zlib,
                    &compressed,
                    plain.len(),
                    &enough::Unstoppable
                )
                .unwrap(),
            plain
        );
    }
    #[test]
    fn known_icc_variants_normalize_unknown_profiles_remain_unchanged() {
        let known = zenpixels_convert::icc_profiles::DISPLAY_P3_V4;
        let mut variant = known.to_vec();
        variant[24..36].fill(23);
        assert_eq!(
            PublishingServices::default()
                .normalize_icc(&variant, 1 << 20, &enough::Unstoppable)
                .unwrap()
                .unwrap(),
            known
        );
        variant[known.len() - 1] ^= 1;
        assert!(
            PublishingServices::default()
                .normalize_icc(&variant, 1 << 20, &enough::Unstoppable)
                .unwrap()
                .is_none()
        );
    }
}

// Merge RDF descriptions using a real XML writer. Values, array kinds, resource
// structure and qualifiers stay in their original shape; XmpValue::Array would
// erase Seq/Alt and language qualifiers if used as a transfer representation.
fn merge_packets(a: &[u8], b: &[u8], limit: usize, stop: &dyn Stop) -> Result<Vec<u8>, Error> {
    use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
    let a = std::str::from_utf8(a).map_err(|_| Error::Malformed("XMP UTF-8"))?;
    let b = std::str::from_utf8(b).map_err(|_| Error::Malformed("XMP UTF-8"))?;
    preflight(a, limit)?;
    preflight(b, limit)?;
    let docs = [
        roxmltree::Document::parse(a).unwrap(),
        roxmltree::Document::parse(b).unwrap(),
    ];
    let mut prefixes = BTreeMap::<String, String>::new();
    // xmpkit 0.1.6 always declares these prefixes in its serializer, even
    // when a parsed packet used aliases. Emit those same names so our own
    // merged packets remain editable without mutating a global registry.
    prefixes.insert(RDF.into(), "rdf".into());
    prefixes.insert(DC.into(), "dc".into());
    prefixes.insert("http://ns.adobe.com/xap/1.0/".into(), "xmp".into());
    prefixes.insert("http://ns.adobe.com/exif/1.0/".into(), "exif".into());
    prefixes.insert("http://www.w3.org/XML/1998/namespace".into(), "xml".into());
    for doc in &docs {
        for n in doc.descendants().filter(|n| n.is_element()) {
            for uri in n
                .tag_name()
                .namespace()
                .into_iter()
                .chain(n.attributes().filter_map(|a| a.namespace()))
            {
                if !prefixes.contains_key(uri) {
                    let next = format!("ns{}", prefixes.len());
                    prefixes.insert(uri.into(), next);
                }
            }
        }
    }
    let writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
        stop,
    };
    let mut writer = quick_xml::Writer::new(writer);
    let map = |_| Error::Service("bounded XML serialization");
    let mut root = BytesStart::new("rdf:RDF");
    let declarations: Vec<_> = prefixes
        .iter()
        .filter(|(_, p)| *p != "xml")
        .map(|(uri, p)| (format!("xmlns:{p}"), uri.clone()))
        .collect();
    for (name, uri) in &declarations {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    writer.write_event(Event::Start(root)).map_err(map)?;
    fn emit(
        n: roxmltree::Node<'_, '_>,
        prefixes: &BTreeMap<String, String>,
        writer: &mut quick_xml::Writer<BoundedWriter<'_>>,
    ) -> Result<(), Error> {
        writer
            .get_ref()
            .stop
            .check()
            .map_err(|_| Error::Cancelled)?;
        let name = |uri: Option<&str>, local: &str| match uri {
            Some(uri) => format!("{}:{local}", prefixes[uri]),
            None => local.to_string(),
        };
        let map = |_| Error::Service("bounded XML serialization");
        if n.is_element() {
            let tag = name(n.tag_name().namespace(), n.tag_name().name());
            let attrs: Vec<_> = n
                .attributes()
                .map(|a| (name(a.namespace(), a.name()), a.value().to_owned()))
                .collect();
            let mut start = BytesStart::new(&tag);
            for (k, v) in &attrs {
                start.push_attribute((k.as_str(), v.as_str()));
            }
            writer.write_event(Event::Start(start)).map_err(map)?;
            for child in n.children() {
                emit(child, prefixes, writer)?;
            }
            writer
                .write_event(Event::End(BytesEnd::new(&tag)))
                .map_err(map)?;
        } else if n.is_text() {
            writer
                .write_event(Event::Text(BytesText::new(n.text().unwrap_or(""))))
                .map_err(map)?;
        }
        // Comments and PIs are descriptive/private; neither supplies a value.
        Ok(())
    }
    for doc in &docs {
        for n in doc.descendants().filter(|n| {
            n.has_tag_name((RDF, "Description"))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        }) {
            emit(n, &prefixes, &mut writer)?;
        }
    }
    writer
        .write_event(Event::End(BytesEnd::new("rdf:RDF")))
        .map_err(map)?;
    let bytes = writer.into_inner().bytes;
    preflight(std::str::from_utf8(&bytes).unwrap(), limit)?;
    Ok(bytes)
}

// Bound the Brotli history window before the decoder allocates it. RFC 7932
// WBITS uses at most seven initial LSB-first bits. Large-window extensions are
// refused. The example allows at most 4 MiB history (also bounded by limit,
// with a 64 KiB minimum decoder workspace); Huffman state is additionally fixed
// by the Brotli format. The output limit is enforced while reading.
fn check_brotli_window(data: &[u8], limit: usize) -> Result<(), Error> {
    let b = *data
        .first()
        .ok_or(Error::Malformed("empty Brotli metadata"))?;
    let bits = if b & 1 == 0 {
        16
    } else {
        let n = (b >> 1) & 7;
        if n != 0 {
            17 + n
        } else {
            let n = (b >> 4) & 7;
            match n {
                0 => 17,
                1 => return Err(Error::Unsupported("large-window Brotli metadata")),
                n => 8 + n,
            }
        }
    };
    if (1usize << bits) > limit.clamp(65536, 4 << 20) {
        return Err(Error::Limit("Brotli history window"));
    }
    Ok(())
}

fn inflate_zlib(data: &[u8], limit: usize, stop: &dyn Stop) -> Result<Vec<u8>, Error> {
    let mut decoder = flate2::Decompress::new(true);
    let mut out = Vec::new();
    let mut buf = [0; 4096];
    loop {
        stop.check().map_err(|_| Error::Cancelled)?;
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let capacity = (limit - out.len()).saturating_add(1).min(buf.len());
        let status = decoder
            .decompress(
                &data[before_in as usize..],
                &mut buf[..capacity],
                flate2::FlushDecompress::None,
            )
            .map_err(|_| Error::Malformed("zlib metadata"))?;
        let n = (decoder.total_out() - before_out) as usize;
        if n > limit - out.len() {
            return Err(Error::Limit("expanded metadata"));
        }
        out.try_reserve(n).map_err(|_| Error::Allocation)?;
        out.extend_from_slice(&buf[..n]);
        if status == flate2::Status::StreamEnd {
            if decoder.total_in() != data.len() as u64 {
                return Err(Error::Malformed("zlib metadata trailer"));
            }
            return Ok(out);
        }
        if n == 0 && decoder.total_in() == before_in {
            return Err(Error::Malformed("truncated zlib metadata"));
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    const REQUIRED: &str = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/" hdrgm:Version="1.0" hdrgm:GainMapMax="2.0"/></rdf:RDF>"#;
    #[test]
    fn merge_keeps_required_signals_and_rights_language_qualifiers() {
        let service = PublishingServices { keep_rights: true };
        let description = service
            .rewrite_xmp(
                super::tests::PACKET.as_bytes(),
                1 << 20,
                &enough::Unstoppable,
            )
            .unwrap()
            .unwrap();
        let merged = service
            .merge_xmp(
                &description,
                REQUIRED.as_bytes(),
                1 << 20,
                &enough::Unstoppable,
            )
            .unwrap();
        let merged = std::str::from_utf8(&merged).unwrap();
        let packet = zencodec::xmp::Packet::parse(merged).unwrap();
        assert_eq!(
            packet
                .property("http://ns.adobe.com/hdr-gain-map/1.0/", "GainMapMax")
                .unwrap()
                .unwrap(),
            ["2.0"]
        );
        assert_eq!(
            packet.property(DC, "creator").unwrap().unwrap(),
            ["A & B", "C"]
        );
        let doc = roxmltree::Document::parse(merged).unwrap();
        assert!(
            doc.descendants().any(|n| n
                .attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                == Some("fr"))
        );
        assert!(!merged.contains("SECRET"));
        // Re-editing our own multi-description output is supported.
        let result = service.rewrite_xmp(merged.as_bytes(), 1 << 20, &enough::Unstoppable);
        assert!(result.is_ok(), "{result:?}\n{merged}");
    }
    #[test]
    fn brotli_roundtrip_truncation_and_expansion_bounds() {
        let plain = vec![42u8; 8192];
        let mut encoded = Vec::new();
        {
            let mut w = brotli::CompressorWriter::new(&mut encoded, 4096, 3, 16);
            w.write_all(&plain).unwrap();
        }
        let service = PublishingServices::default();
        assert_eq!(
            service
                .decompress(Compression::Brotli, &encoded, 16384, &enough::Unstoppable)
                .unwrap(),
            plain
        );
        assert!(
            service
                .decompress(Compression::Brotli, &encoded, 1024, &enough::Unstoppable)
                .is_err()
        );
        for end in 0..encoded.len() {
            assert!(
                service
                    .decompress(
                        Compression::Brotli,
                        &encoded[..end],
                        16384,
                        &enough::Unstoppable
                    )
                    .is_err(),
                "{end}"
            );
        }
    }
    #[test]
    fn zlib_truncation_and_trailer_refuse() {
        let service = PublishingServices::default();
        let plain = vec![5u8; 8192];
        let compressed = service
            .compress(Compression::Zlib, &plain, 16384, &enough::Unstoppable)
            .unwrap();
        for end in 0..compressed.len() {
            assert!(
                service
                    .decompress(
                        Compression::Zlib,
                        &compressed[..end],
                        16384,
                        &enough::Unstoppable
                    )
                    .is_err(),
                "{end}"
            );
        }
        let mut trailing = compressed.clone();
        trailing.extend_from_slice(b"SECRET");
        assert!(
            service
                .decompress(Compression::Zlib, &trailing, 16384, &enough::Unstoppable)
                .is_err()
        );
    }
    #[test]
    fn large_brotli_windows_refuse_before_allocation() {
        assert!(check_brotli_window(&[0x11], 16 << 20).is_err());
        assert!(check_brotli_window(&[0x0f], 16 << 20).is_err()); // 24-bit window
        assert!(check_brotli_window(&[0], 65536).is_ok());
    }
    #[test]
    fn unknown_uri_cannot_survive_publishing_unreported() {
        for uri in ["https://example.org/private/", "urn:private:device"] {
            let xml = super::tests::PACKET.replace("http://example.org/private/", uri);
            let result = PublishingServices { keep_rights: true }.rewrite_xmp(
                xml.as_bytes(),
                1 << 20,
                &enough::Unstoppable,
            );
            if let Ok(Some(bytes)) = result {
                assert!(!String::from_utf8(bytes).unwrap().contains("SECRET"));
            }
        }
    }
}
