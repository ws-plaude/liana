use crate::widget::Image;
use iced::{widget::image::Handle, window::icon, Length};

// Logos and illustrations are pre-rendered from the SVG sources next to them by
// contrib/prerender_svg.py, so the app ships no SVG renderer.
const LIANA_APP_ICON: &[u8] = include_bytes!("../static/logos/liana-app-icon.png");
const LIANA_BUSINESS_APP_ICON: &[u8] =
    include_bytes!("../static/logos/liana-business-app-icon.png");
const LIANA_LOGO_GREY: &[u8] = include_bytes!("../static/logos/LIANA_SYMBOL_Gray.png");
const LIANA_LOGO_GREEN: &[u8] = include_bytes!("../static/logos/LIANA_SYMBOL_Green.png");
const LIANA_LOGO_BLUE: &[u8] = include_bytes!("../static/logos/LIANA_SYMBOL_Blue.png");
const LIANA_BRAND_GREY: &[u8] = include_bytes!("../static/logos/LIANA_BRAND_Gray.png");
const LIANA_WALLET_LOGO: &[u8] = include_bytes!("../static/logos/liana_wallet.png");
const LIANA_BUSINESS_LOGO: &[u8] = include_bytes!("../static/logos/liana_business.png");
const WIZARDSARDINE_LETTERING: &[u8] = include_bytes!("../static/logos/logo-wizardsardine.png");

fn icon_from_png(data: &[u8]) -> icon::Icon {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8();
    let (width, height) = img.dimensions();
    icon::from_rgba(img.into_raw(), width, height).unwrap()
}

// Fill the available width like the svg widget used to, and keep the height
// derived from the aspect ratio.
fn image_from_png(data: &'static [u8]) -> Image {
    Image::new(Handle::from_bytes(data)).width(Length::Fill)
}

pub fn liana_app_icon() -> icon::Icon {
    icon_from_png(LIANA_APP_ICON)
}

pub fn liana_business_app_icon() -> icon::Icon {
    icon_from_png(LIANA_BUSINESS_APP_ICON)
}

pub fn liana_grey_logo() -> Image {
    image_from_png(LIANA_LOGO_GREY)
}

pub fn liana_green_logo() -> Image {
    image_from_png(LIANA_LOGO_GREEN)
}

pub fn liana_blue_logo() -> Image {
    image_from_png(LIANA_LOGO_BLUE)
}

pub fn liana_brand_grey() -> Image {
    image_from_png(LIANA_BRAND_GREY)
}

pub fn liana_wallet_logo() -> Image {
    image_from_png(LIANA_WALLET_LOGO)
}

pub fn liana_business_logo() -> Image {
    image_from_png(LIANA_BUSINESS_LOGO)
}

pub fn wizardsardine() -> Image {
    image_from_png(WIZARDSARDINE_LETTERING)
}

const CREATE_NEW_WALLET_ICON: &[u8] = include_bytes!("../static/icons/blueprint.png");

pub fn create_new_wallet_icon() -> Image {
    image_from_png(CREATE_NEW_WALLET_ICON)
}

const PARTICIPATE_IN_NEW_WALLET_ICON: &[u8] = include_bytes!("../static/icons/discussion.png");

pub fn participate_in_new_wallet_icon() -> Image {
    image_from_png(PARTICIPATE_IN_NEW_WALLET_ICON)
}

const RESTORE_WALLET_ICON: &[u8] = include_bytes!("../static/icons/syncdata.png");

pub fn restore_wallet_icon() -> Image {
    image_from_png(RESTORE_WALLET_ICON)
}

const SUCCESS_MARK_ICON: &[u8] = include_bytes!("../static/icons/success-mark.png");

pub fn success_mark_icon() -> Image {
    image_from_png(SUCCESS_MARK_ICON)
}

const KEY_MARK_ICON: &[u8] = include_bytes!("../static/icons/key-mark.png");

pub fn key_mark_icon() -> Image {
    image_from_png(KEY_MARK_ICON)
}

const INHERITANCE_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/inheritance_template_description.png");

pub fn inheritance_template_description() -> Image {
    image_from_png(INHERITANCE_TEMPLATE_DESC)
}

const CUSTOM_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/custom_template_description.png");

pub fn custom_template_description() -> Image {
    image_from_png(CUSTOM_TEMPLATE_DESC)
}

const MULTISIG_SECURITY_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/multisig_security_template.png");

pub fn multisig_security_template_description() -> Image {
    image_from_png(MULTISIG_SECURITY_TEMPLATE_DESC)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The pre-rendered assets must stay decodable by the codecs we enable.
    #[test]
    fn assets_decode_as_png() {
        for (name, data) in [
            ("liana app icon", LIANA_APP_ICON),
            ("liana business app icon", LIANA_BUSINESS_APP_ICON),
            ("grey logo", LIANA_LOGO_GREY),
            ("green logo", LIANA_LOGO_GREEN),
            ("blue logo", LIANA_LOGO_BLUE),
            ("grey brand", LIANA_BRAND_GREY),
            ("wallet logo", LIANA_WALLET_LOGO),
            ("business logo", LIANA_BUSINESS_LOGO),
            ("wizardsardine lettering", WIZARDSARDINE_LETTERING),
            ("create new wallet", CREATE_NEW_WALLET_ICON),
            ("participate in new wallet", PARTICIPATE_IN_NEW_WALLET_ICON),
            ("restore wallet", RESTORE_WALLET_ICON),
            ("success mark", SUCCESS_MARK_ICON),
            ("key mark", KEY_MARK_ICON),
            ("inheritance template", INHERITANCE_TEMPLATE_DESC),
            ("custom template", CUSTOM_TEMPLATE_DESC),
            (
                "multisig security template",
                MULTISIG_SECURITY_TEMPLATE_DESC,
            ),
        ] {
            let decoded = image::load_from_memory_with_format(data, image::ImageFormat::Png)
                .unwrap_or_else(|e| panic!("decoding {name}: {e}"));
            assert!(
                decoded.width() > 0 && decoded.height() > 0,
                "{name} is empty"
            );
        }
    }
}
