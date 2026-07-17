use std::io::Cursor;

use ::image::{DynamicImage, ImageFormat};
use nostr_sdk::prelude::*;

use super::{decode_image, nip08_mentions, short_id, CACHED_IMAGE_SIZE};

#[test]
fn short_ids_are_safe() {
    assert_eq!(short_id("123456789"), "12345678");
    assert_eq!(short_id("短い"), "短い");
}

#[test]
fn encodes_nip19_mentions_as_nip08_tags() {
    let npub = Keys::generate().public_key().to_bech32().unwrap();
    let (content, tags) = nip08_mentions(&format!("hello @{npub} and @{npub}!"));
    assert_eq!(content, "hello #[0] and #[0]!");
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].kind(), TagKind::p());
}

#[test]
fn leaves_invalid_mentions_unchanged() {
    let (content, tags) = nip08_mentions("hello @npub1invalid");
    assert_eq!(content, "hello @npub1invalid");
    assert!(tags.is_empty());
}

#[test]
fn image_decoder_normalizes_retained_size() {
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::new_rgba8(512, 256)
        .write_to(&mut encoded, ImageFormat::Png)
        .unwrap();

    let decoded = decode_image(encoded.into_inner()).unwrap();
    assert!(decoded.width() <= CACHED_IMAGE_SIZE);
    assert!(decoded.height() <= CACHED_IMAGE_SIZE);
}

#[test]
fn image_decoder_rejects_excessive_dimensions() {
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::new_rgba8(2_049, 1)
        .write_to(&mut encoded, ImageFormat::Png)
        .unwrap();

    assert!(decode_image(encoded.into_inner()).is_err());
}

#[test]
fn image_decoder_rasterizes_svg_custom_emoji() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16">
            <rect width="32" height="16" fill="#ff0000"/>
        </svg>"##;

    let decoded = decode_image(svg.to_vec()).unwrap();

    assert_eq!(decoded.width(), CACHED_IMAGE_SIZE);
    assert_eq!(decoded.height(), CACHED_IMAGE_SIZE / 2);
    let pixel = decoded.to_rgba8().get_pixel(64, 32).0;
    assert!(pixel[0] > 240);
    assert!(pixel[1] < 16);
    assert_eq!(pixel[3], 255);
}

#[test]
fn image_decoder_rejects_oversized_svg_dimensions() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2049" height="16"/>"#;
    assert!(decode_image(svg.to_vec()).is_err());
}
