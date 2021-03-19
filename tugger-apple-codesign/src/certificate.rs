// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Functionality related to certificates.

use {
    bcder::{
        encode::{PrimitiveContent, Values},
        BitString, ConstOid, Mode, OctetString, Oid,
    },
    bytes::Bytes,
    cryptographic_message_syntax::{
        asn1::{
            common::Time,
            rfc5280::{
                AlgorithmIdentifier, Certificate, Extension, Extensions, SubjectPublicKeyInfo,
                TbsCertificate, Validity, Version,
            },
            rfc5958::OneAsymmetricKey,
        },
        CmsError, RelativeDistinguishedName, SignatureAlgorithm, SigningKey,
    },
    ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING},
};

/// Key Usage extension.
///
/// 2.5.29.15
const OID_EXTENSION_KEY_USAGE: ConstOid = Oid(&[85, 29, 15]);

/// Extended Key Usage extension.
///
/// 2.5.29.37
const OID_EXTENSION_EXTENDED_KEY_USAGE: ConstOid = Oid(&[85, 29, 37]);

/// Extended Key Usage purpose for code signing.
///
/// 1.3.6.1.5.5.7.3.3
const OID_PURPOSE_CODE_SIGNING: ConstOid = Oid(&[43, 6, 1, 5, 5, 7, 3, 3]);

/// OID used for email address in RDN in Apple generated code signing certificates.
const OID_EMAIL_ADDRESS: ConstOid = Oid(&[42, 134, 72, 134, 247, 13, 1, 9, 1]);

#[derive(Debug)]
pub enum CertificateError {
    /// An unspecified error in ring.
    Ring(ring::error::Unspecified),

    /// Error decoding ASN.1.
    Asn1Decode(bcder::decode::Error),

    /// I/O error.
    Io(std::io::Error),

    /// Error in cryptographic message syntax crate.
    Cms(CmsError),

    /// Bad string value.
    Charset(bcder::string::CharSetError),
}

impl std::fmt::Display for CertificateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ring(e) => f.write_fmt(format_args!("error in ring: {}", e)),
            Self::Asn1Decode(e) => f.write_fmt(format_args!("error decoding ASN.1: {}", e)),
            Self::Io(e) => f.write_fmt(format_args!("I/O error: {}", e)),
            Self::Cms(e) => f.write_fmt(format_args!("CMS error: {}", e)),
            Self::Charset(e) => f.write_fmt(format_args!("bad string value: {:?}", e)),
        }
    }
}

impl std::error::Error for CertificateError {}

impl From<ring::error::Unspecified> for CertificateError {
    fn from(e: ring::error::Unspecified) -> Self {
        Self::Ring(e)
    }
}

impl From<bcder::decode::Error> for CertificateError {
    fn from(e: bcder::decode::Error) -> Self {
        Self::Asn1Decode(e)
    }
}

impl From<std::io::Error> for CertificateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<CmsError> for CertificateError {
    fn from(e: CmsError) -> Self {
        Self::Cms(e)
    }
}

impl From<bcder::string::CharSetError> for CertificateError {
    fn from(e: bcder::string::CharSetError) -> Self {
        Self::Charset(e)
    }
}

/// Create a new self-signed X.509 certificate suitable for signing code.
///
/// The created certificate contains all the extensions needed to convey
/// that it is used for code signing and should resemble certificates.
///
/// However, because the certificate isn't signed by Apple or another
/// trusted certificate authority, binaries signed with the certificate
/// may not pass Apple's verification requirements and the OS may refuse
/// to proceed. Needless to say, only use certificates generated with this
/// function for testing purposes only.
pub fn create_self_signed_code_signing_certificate(
    common_name: &str,
    country_name: &str,
    email_address: &str,
    validity_duration: chrono::Duration,
) -> Result<(cryptographic_message_syntax::Certificate, SigningKey), CertificateError> {
    let system_random = ring::rand::SystemRandom::new();
    let signing_algorithm = &ECDSA_P256_SHA256_ASN1_SIGNING;

    let key_pair_document = EcdsaKeyPair::generate_pkcs8(signing_algorithm, &system_random)?;

    let key_pair_asn1 =
        bcder::decode::Constructed::decode(key_pair_document.as_ref(), Mode::Der, |cons| {
            OneAsymmetricKey::take_from(cons)
        })?;

    let key_pair = EcdsaKeyPair::from_pkcs8(signing_algorithm, key_pair_document.as_ref())
        .expect("why would ring reject a key pair it just generated?");

    let mut rdn = RelativeDistinguishedName::default();
    rdn.set_common_name(common_name)?;
    rdn.set_country_name(country_name)?;
    rdn.set_attribute_string(Oid(Bytes::from(OID_EMAIL_ADDRESS.as_ref())), email_address)?;

    let now = chrono::Utc::now();
    let expires = now + validity_duration;

    let mut extensions = Extensions::default();

    // Digital Signature key usage extension.
    extensions.push(Extension {
        id: Oid(Bytes::from(OID_EXTENSION_KEY_USAGE.as_ref())),
        critical: Some(true),
        value: OctetString::new(Bytes::copy_from_slice(&[3, 2, 7, 128])),
    });

    let captured =
        bcder::encode::sequence(Oid(Bytes::from(OID_PURPOSE_CODE_SIGNING.as_ref())).encode())
            .to_captured(Mode::Ber);

    extensions.push(Extension {
        id: Oid(Bytes::from(OID_EXTENSION_EXTENDED_KEY_USAGE.as_ref())),
        critical: Some(true),
        value: OctetString::new(Bytes::copy_from_slice(captured.as_ref())),
    });

    let tbs_certificate = TbsCertificate {
        version: Version::V3,
        serial_number: 42.into(),
        signature: AlgorithmIdentifier {
            algorithm: SignatureAlgorithm::EcdsaSha256.into(),
            parameters: None,
        },
        issuer: rdn.clone().into(),
        validity: Validity {
            not_before: Time::from(now),
            not_after: Time::from(expires),
        },
        subject: rdn.into(),
        subject_public_key_info: SubjectPublicKeyInfo {
            algorithm: AlgorithmIdentifier {
                algorithm: key_pair_asn1.private_key_algorithm.algorithm.clone(),
                parameters: key_pair_asn1.private_key_algorithm.parameters.clone(),
            },
            subject_public_key: BitString::new(
                0,
                Bytes::copy_from_slice(key_pair.public_key().as_ref()),
            ),
        },
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: Some(extensions),
    };

    // We need to serialize the TBS certificate so we can sign it with the private
    // key and include its signature.
    let mut cert_ber = Vec::<u8>::new();
    tbs_certificate
        .encode_ref()
        .write_encoded(Mode::Ber, &mut cert_ber)?;

    let signature = key_pair.sign(&system_random, &cert_ber)?;

    let cert = Certificate {
        tbs_certificate,
        signature_algorithm: AlgorithmIdentifier {
            algorithm: SignatureAlgorithm::EcdsaSha256.into(),
            parameters: None,
        },
        signature: BitString::new(0, Bytes::copy_from_slice(signature.as_ref())),
    };

    let cert = cryptographic_message_syntax::Certificate::from_parsed_asn1(cert)?;

    Ok((cert, key_pair.into()))
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        cryptographic_message_syntax::{SignedData, SignedDataBuilder, SignerBuilder},
    };

    #[test]
    fn generate_self_signed_certificate() {
        create_self_signed_code_signing_certificate(
            "test",
            "US",
            "nobody@example.com",
            chrono::Duration::hours(1),
        )
        .unwrap();
    }

    #[test]
    fn cms_self_signed_certificate_signing() {
        let (cert, signing_key) = create_self_signed_code_signing_certificate(
            "test",
            "US",
            "nobody@example.com",
            chrono::Duration::hours(1),
        )
        .unwrap();

        let plaintext = "hello, world";

        let cms = SignedDataBuilder::default()
            .certificate(cert.clone())
            .unwrap()
            .signed_content(plaintext.as_bytes().to_vec())
            .signer(SignerBuilder::new(&signing_key, cert))
            .build_ber()
            .unwrap();

        let signed_data = SignedData::parse_ber(&cms).unwrap();

        for signer in signed_data.signers() {
            signer
                .verify_signature_with_signed_data(&signed_data)
                .unwrap();
        }
    }
}