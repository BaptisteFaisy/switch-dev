use reqwest::{redirect::Policy, Client, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fmt, time::Duration};
use url::Url;

const DEFAULT_DUELLO_DASHBOARD_URL: &str = "https://duello.fr/dashboard";
const STRIPE_BALANCE_DASHBOARD_URL: &str = "https://dashboard.stripe.com/balance";
const MAX_WALLETS: usize = 500;
const MAX_WALLETS_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_CREDIT_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_CREDIT_MINOR: i64 = 10_000_000;

#[derive(Clone)]
enum DuelloBankConfiguration {
    Disabled,
    Invalid(String),
    Ready {
        api_origin: String,
        admin_token: String,
    },
}

/// Configuration lue avant la creation du runtime. Le `Debug` manuel ne doit
/// jamais exposer le jeton Duello dans un diagnostic de `ServerConfig`.
#[derive(Clone)]
pub struct DuelloBankConfig {
    state: DuelloBankConfiguration,
    dashboard_url: String,
}

impl fmt::Debug for DuelloBankConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.state {
            DuelloBankConfiguration::Disabled => "disabled",
            DuelloBankConfiguration::Invalid(_) => "invalid",
            DuelloBankConfiguration::Ready { .. } => "ready",
        };
        formatter
            .debug_struct("DuelloBankConfig")
            .field("state", &state)
            .field("dashboard_url", &self.dashboard_url)
            .finish()
    }
}

impl DuelloBankConfig {
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("CST_DUELLO_BANK_API_ORIGIN").ok().as_deref(),
            std::env::var("CST_DUELLO_BANK_ADMIN_TOKEN").ok().as_deref(),
            std::env::var("CST_DUELLO_DASHBOARD_URL").ok().as_deref(),
        )
    }

    pub(crate) fn from_values(
        raw_api_origin: Option<&str>,
        raw_admin_token: Option<&str>,
        raw_dashboard_url: Option<&str>,
    ) -> Self {
        let dashboard_url = validated_dashboard_url(raw_dashboard_url)
            .unwrap_or_else(|| DEFAULT_DUELLO_DASHBOARD_URL.to_string());
        let api_origin = raw_api_origin
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let admin_token = raw_admin_token
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let state = match (api_origin, admin_token) {
            (None, None) => DuelloBankConfiguration::Disabled,
            (None, Some(_)) => DuelloBankConfiguration::Invalid(
                "CST_DUELLO_BANK_API_ORIGIN manque alors que le jeton Duello est defini."
                    .to_string(),
            ),
            (Some(_), None) => DuelloBankConfiguration::Invalid(
                "CST_DUELLO_BANK_ADMIN_TOKEN manque alors que l'origine Duello est definie."
                    .to_string(),
            ),
            (Some(origin), Some(token)) => match validated_api_origin(origin) {
                Some(api_origin) if valid_admin_token(token) => DuelloBankConfiguration::Ready {
                    api_origin,
                    admin_token: token.to_string(),
                },
                None => DuelloBankConfiguration::Invalid(
                    "CST_DUELLO_BANK_API_ORIGIN doit etre une origine HTTPS sans chemin ni identifiants (HTTP est accepte uniquement en boucle locale).".to_string(),
                ),
                Some(_) => DuelloBankConfiguration::Invalid(
                    "CST_DUELLO_BANK_ADMIN_TOKEN doit contenir entre 32 et 512 caracteres ASCII visibles.".to_string(),
                ),
            },
        };

        Self {
            state,
            dashboard_url,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuelloBankErrorKind {
    BadRequest,
    NotFound,
    Conflict,
    TooManyRequests,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct DuelloBankError {
    pub kind: DuelloBankErrorKind,
    pub message: String,
}

impl DuelloBankError {
    fn new(kind: DuelloBankErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(DuelloBankErrorKind::Unavailable, message)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuelloBankWallet {
    pub public_id: String,
    pub referral_code: String,
    pub display_name: String,
    pub email: String,
    pub click_count: u64,
    pub credited_click_count: u64,
    pub available_minor: u64,
    pub currency: String,
    pub stripe_status: Option<String>,
    pub stripe_payouts_enabled: Option<bool>,
    pub can_withdraw: Option<bool>,
    /// Adresse Solana du portefeuille du membre, fournie par Duello
    /// (champ `solanaAddress` du wallet). Optionnelle tant que le membre
    /// n'a pas renseigne son adresse de retrait Phantom/USDC.
    pub solana_address: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuelloBankSnapshot {
    pub configured: bool,
    pub configuration_message: Option<String>,
    pub dashboard_url: String,
    pub stripe_dashboard_url: String,
    pub wallets: Vec<DuelloBankWallet>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreditDuelloWalletRequest {
    pub public_id: String,
    pub amount_minor: i64,
    pub reason: String,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditDuelloWalletResult {
    pub success: bool,
    pub idempotent: bool,
}

#[derive(Clone)]
pub struct DuelloBankClient {
    config: DuelloBankConfig,
    http: Client,
}

impl DuelloBankClient {
    pub fn new(config: DuelloBankConfig) -> Result<Self, String> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(8))
            .redirect(Policy::none())
            .user_agent("Codex-Switch-Terminal/Duello-Bank")
            .build()
            .map_err(|error| format!("Client Banque Duello impossible: {error}"))?;
        Ok(Self { config, http })
    }

    pub async fn snapshot(&self) -> Result<DuelloBankSnapshot, DuelloBankError> {
        match &self.config.state {
            DuelloBankConfiguration::Disabled => Ok(self.configuration_snapshot(Some(
                "Ajoutez l'origine privee et le jeton administrateur Duello dans la configuration locale de Switch.".to_string(),
            ))),
            DuelloBankConfiguration::Invalid(message) => {
                Ok(self.configuration_snapshot(Some(message.clone())))
            }
            DuelloBankConfiguration::Ready { .. } => Ok(DuelloBankSnapshot {
                configured: true,
                configuration_message: None,
                dashboard_url: self.config.dashboard_url.clone(),
                stripe_dashboard_url: STRIPE_BALANCE_DASHBOARD_URL.to_string(),
                wallets: self.fetch_wallets().await?,
            }),
        }
    }

    pub async fn credit(
        &self,
        request: CreditDuelloWalletRequest,
    ) -> Result<CreditDuelloWalletResult, DuelloBankError> {
        let validated = validate_credit_request(request)?;
        let (api_origin, admin_token) = self.ready_configuration()?;

        // Les metadonnees Duello sont relues cote serveur : le navigateur ne
        // choisit ni le slug ni le snapshot de clics transmis au grand livre.
        let wallet = self
            .fetch_wallets()
            .await?
            .into_iter()
            .find(|wallet| wallet.public_id == validated.public_id)
            .ok_or_else(|| {
                DuelloBankError::new(
                    DuelloBankErrorKind::NotFound,
                    "Le portefeuille Duello selectionne est introuvable.",
                )
            })?;
        if wallet.currency != "EUR" {
            return Err(DuelloBankError::new(
                DuelloBankErrorKind::BadRequest,
                "Seuls les portefeuilles Duello en EUR peuvent etre credites depuis cette vue.",
            ));
        }

        let payload = json!({
            "publicId": wallet.public_id,
            "amountMinor": validated.amount_minor,
            "clickCountSnapshot": wallet.click_count,
            "linkSlug": wallet.referral_code,
            "reason": validated.reason,
            "reference": validated.reference,
        });
        let response = self
            .http
            .post(format!("{api_origin}/api/admin/affiliate-credits"))
            .bearer_auth(admin_token)
            .header("Accept", "application/json")
            .header(
                "Idempotency-Key",
                credit_idempotency_key(&validated.reference),
            )
            .json(&payload)
            .send()
            .await
            .map_err(|_| {
                DuelloBankError::unavailable(
                    "Le service de credit Duello est momentanement injoignable.",
                )
            })?;

        if !response.status().is_success() {
            return Err(map_credit_status(response.status().as_u16()));
        }
        let body = read_json_within_limit(response, MAX_CREDIT_RESPONSE_BYTES).await?;
        let idempotent = body
            .get("idempotent")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                DuelloBankError::unavailable(
                    "Le service de credit Duello a renvoye une reponse invalide.",
                )
            })?;
        if !body.get("credit").is_some_and(Value::is_object)
            || !body.get("wallet").is_some_and(Value::is_object)
        {
            return Err(DuelloBankError::unavailable(
                "Le service de credit Duello a renvoye une reponse invalide.",
            ));
        }
        Ok(CreditDuelloWalletResult {
            success: true,
            idempotent,
        })
    }

    fn configuration_snapshot(&self, message: Option<String>) -> DuelloBankSnapshot {
        DuelloBankSnapshot {
            configured: false,
            configuration_message: message,
            dashboard_url: self.config.dashboard_url.clone(),
            stripe_dashboard_url: STRIPE_BALANCE_DASHBOARD_URL.to_string(),
            wallets: Vec::new(),
        }
    }

    fn ready_configuration(&self) -> Result<(&str, &str), DuelloBankError> {
        match &self.config.state {
            DuelloBankConfiguration::Ready {
                api_origin,
                admin_token,
            } => Ok((api_origin, admin_token)),
            DuelloBankConfiguration::Disabled => Err(DuelloBankError::unavailable(
                "La Banque Duello n'est pas encore configuree dans Switch.",
            )),
            DuelloBankConfiguration::Invalid(message) => {
                Err(DuelloBankError::unavailable(message.clone()))
            }
        }
    }

    async fn fetch_wallets(&self) -> Result<Vec<DuelloBankWallet>, DuelloBankError> {
        let (api_origin, admin_token) = self.ready_configuration()?;
        let response = self
            .http
            .get(format!("{api_origin}/api/admin/affiliate-wallets"))
            .bearer_auth(admin_token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| {
                DuelloBankError::unavailable(
                    "La liste des portefeuilles Duello est momentanement injoignable.",
                )
            })?;
        if !response.status().is_success() {
            return Err(map_wallet_status(response.status().as_u16()));
        }
        let body = read_json_within_limit(response, MAX_WALLETS_RESPONSE_BYTES).await?;
        parse_wallets(&body)
    }
}

#[derive(Debug)]
struct ValidatedCredit {
    public_id: String,
    amount_minor: u64,
    reason: String,
    reference: String,
}

fn validate_credit_request(
    request: CreditDuelloWalletRequest,
) -> Result<ValidatedCredit, DuelloBankError> {
    let public_id = request.public_id.trim().to_ascii_lowercase();
    if !valid_member_id(&public_id) {
        return Err(DuelloBankError::new(
            DuelloBankErrorKind::BadRequest,
            "L'identifiant public Duello est invalide.",
        ));
    }
    if !(1..=MAX_CREDIT_MINOR).contains(&request.amount_minor) {
        return Err(DuelloBankError::new(
            DuelloBankErrorKind::BadRequest,
            "Le montant doit etre compris entre 0,01 EUR et 100 000,00 EUR.",
        ));
    }
    let reason = normalized_single_line(&request.reason, 3, 240).ok_or_else(|| {
        DuelloBankError::new(
            DuelloBankErrorKind::BadRequest,
            "Le motif doit contenir entre 3 et 240 caracteres sur une seule ligne.",
        )
    })?;
    let reference = normalized_single_line(&request.reference, 3, 120).ok_or_else(|| {
        DuelloBankError::new(
            DuelloBankErrorKind::BadRequest,
            "La reference doit contenir entre 3 et 120 caracteres sur une seule ligne.",
        )
    })?;
    if !valid_reference(&reference) {
        return Err(DuelloBankError::new(
            DuelloBankErrorKind::BadRequest,
            "La reference de credit contient un caractere non autorise.",
        ));
    }
    Ok(ValidatedCredit {
        public_id,
        amount_minor: request.amount_minor as u64,
        reason,
        reference,
    })
}

fn parse_wallets(body: &Value) -> Result<Vec<DuelloBankWallet>, DuelloBankError> {
    let candidates = body
        .get("wallets")
        .and_then(Value::as_array)
        .filter(|wallets| wallets.len() <= MAX_WALLETS)
        .ok_or_else(invalid_wallet_response)?;
    let mut wallets = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let object = candidate.as_object().ok_or_else(invalid_wallet_response)?;
        let public_id =
            required_text(object.get("publicId"), 72).ok_or_else(invalid_wallet_response)?;
        let referral_code =
            required_text(object.get("referralCode"), 72).ok_or_else(invalid_wallet_response)?;
        let display_name =
            required_text(object.get("displayName"), 120).ok_or_else(invalid_wallet_response)?;
        let email = required_text(object.get("email"), 320).ok_or_else(invalid_wallet_response)?;
        let currency = required_text(object.get("currency"), 3)
            .map(|value| value.to_ascii_uppercase())
            .filter(|value| value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()))
            .ok_or_else(invalid_wallet_response)?;
        let click_count = object
            .get("clickCount")
            .and_then(Value::as_u64)
            .ok_or_else(invalid_wallet_response)?;
        let credited_click_count = object
            .get("creditedClickCount")
            .and_then(Value::as_u64)
            .filter(|value| *value <= click_count)
            .ok_or_else(invalid_wallet_response)?;
        let available_minor = object
            .get("availableMinor")
            .and_then(Value::as_u64)
            .ok_or_else(invalid_wallet_response)?;
        if !valid_member_id(&public_id) || !valid_member_id(&referral_code) {
            return Err(invalid_wallet_response());
        }

        let connection = object.get("connection").and_then(Value::as_object);
        let stripe_status = connection
            .and_then(|value| value.get("status"))
            .and_then(|value| required_text(Some(value), 64));
        let stripe_payouts_enabled = connection
            .and_then(|value| value.get("payoutsEnabled"))
            .and_then(Value::as_bool);
        let can_withdraw = object.get("canWithdraw").and_then(Value::as_bool);
        let solana_address = object
            .get("solanaAddress")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(validated_solana_address)
            .transpose()?;
        wallets.push(DuelloBankWallet {
            public_id,
            referral_code,
            display_name,
            email,
            click_count,
            credited_click_count,
            available_minor,
            currency,
            stripe_status,
            stripe_payouts_enabled,
            can_withdraw,
            solana_address,
        });
    }
    wallets.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
            .then_with(|| left.public_id.cmp(&right.public_id))
    });
    Ok(wallets)
}

async fn read_json_within_limit(
    mut response: Response,
    limit: usize,
) -> Result<Value, DuelloBankError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(DuelloBankError::unavailable(
            "Le service Duello a renvoye une reponse trop volumineuse.",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DuelloBankError::unavailable("La reponse du service Duello est illisible."))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(DuelloBankError::unavailable(
                "Le service Duello a renvoye une reponse trop volumineuse.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| {
        DuelloBankError::unavailable("Le service Duello a renvoye une reponse invalide.")
    })
}

fn map_wallet_status(status: u16) -> DuelloBankError {
    match status {
        401 | 403 => DuelloBankError::unavailable(
            "Le jeton administrateur de la Banque Duello a ete refuse.",
        ),
        429 => DuelloBankError::new(
            DuelloBankErrorKind::TooManyRequests,
            "Duello limite temporairement les actualisations. Reessayez dans quelques instants.",
        ),
        _ => DuelloBankError::unavailable(
            "La liste des portefeuilles Duello est momentanement indisponible.",
        ),
    }
}

fn map_credit_status(status: u16) -> DuelloBankError {
    match status {
        400 | 422 => DuelloBankError::new(
            DuelloBankErrorKind::BadRequest,
            "Le credit a ete refuse car ses donnees sont invalides.",
        ),
        401 | 403 => DuelloBankError::unavailable(
            "Le jeton administrateur de la Banque Duello a ete refuse.",
        ),
        404 => DuelloBankError::new(
            DuelloBankErrorKind::NotFound,
            "Le membre ou le portefeuille Duello est introuvable.",
        ),
        409 => DuelloBankError::new(
            DuelloBankErrorKind::Conflict,
            "Cette reference existe deja avec des donnees differentes.",
        ),
        429 => DuelloBankError::new(
            DuelloBankErrorKind::TooManyRequests,
            "Trop de credits ont ete demandes. Reessayez dans quelques instants.",
        ),
        _ => DuelloBankError::unavailable(
            "Le service de credit Duello est momentanement indisponible.",
        ),
    }
}

fn invalid_wallet_response() -> DuelloBankError {
    DuelloBankError::unavailable("Le service Duello a renvoye une liste de portefeuilles invalide.")
}

fn validated_api_origin(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    let host = url.host_str()?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .ok()
            .is_some_and(|address| address.is_loopback());
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || (url.path() != "/" && !url.path().is_empty())
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn validated_dashboard_url(raw: Option<&str>) -> Option<String> {
    let value = raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DUELLO_DASHBOARD_URL);
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.to_string().trim_end_matches('/').to_string())
}

fn valid_admin_token(value: &str) -> bool {
    (32..=512).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

fn valid_member_id(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("member-") else {
        return false;
    };
    (1..=64).contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Valide la representation base58 d'une cle publique Solana de 32 octets.
/// La classification on-chain (wallet System Program, et non compte de
/// programme) reste faite juste avant l'envoi par le client Solana.
fn validated_solana_address(value: &str) -> Result<String, DuelloBankError> {
    if !(32..=44).contains(&value.len()) {
        return Err(DuelloBankError::unavailable(
            "L'adresse Solana d'un portefeuille Duello est invalide.",
        ));
    }
    let decoded = bs58::decode(value).into_vec().map_err(|_| {
        DuelloBankError::unavailable("L'adresse Solana d'un portefeuille Duello est invalide.")
    })?;
    if decoded.len() != 32 {
        return Err(DuelloBankError::unavailable(
            "L'adresse Solana d'un portefeuille Duello est invalide.",
        ));
    }
    Ok(value.to_string())
}

fn required_text(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let value = value?.as_str()?.trim();
    let count = value.chars().count();
    if count == 0 || count > max_chars || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

fn normalized_single_line(value: &str, min_chars: usize, max_chars: usize) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = normalized.chars().count();
    (min_chars..=max_chars)
        .contains(&count)
        .then_some(normalized)
}

fn valid_reference(value: &str) -> bool {
    value.chars().next().is_some_and(char::is_alphanumeric)
        && value.chars().all(|character| {
            character.is_alphanumeric()
                || character == ' '
                || matches!(character, '.' | '_' | ':' | '/' | '-')
        })
}

fn credit_idempotency_key(reference: &str) -> String {
    let normalized = reference.trim().to_lowercase();
    let digest = Sha256::digest(format!("duello-affiliate-credit-v1:{normalized}").as_bytes());
    format!("affcred_{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_config() -> DuelloBankConfig {
        DuelloBankConfig::from_values(
            Some("https://duello-api.example.test"),
            Some(&"s".repeat(40)),
            Some("https://duello.fr/dashboard"),
        )
    }

    #[test]
    fn configuration_is_disabled_without_secret_and_rejects_unsafe_origins() {
        assert!(matches!(
            DuelloBankConfig::from_values(None, None, None).state,
            DuelloBankConfiguration::Disabled
        ));
        assert!(matches!(
            DuelloBankConfig::from_values(
                Some("http://public.example.test"),
                Some(&"s".repeat(40)),
                None,
            )
            .state,
            DuelloBankConfiguration::Invalid(_)
        ));
        assert!(matches!(
            ready_config().state,
            DuelloBankConfiguration::Ready { .. }
        ));
    }

    #[test]
    fn validates_credit_fields_and_stable_idempotency() {
        let valid = validate_credit_request(CreditDuelloWalletRequest {
            public_id: "member-a1b2".into(),
            amount_minor: 1_250,
            reason: "Prime campagne aout".into(),
            reference: "switch-20260826-a1b2".into(),
        })
        .unwrap();
        assert_eq!(valid.amount_minor, 1_250);
        assert_eq!(
            credit_idempotency_key("Switch-20260826-A1B2"),
            credit_idempotency_key("switch-20260826-a1b2")
        );
        assert!(validate_credit_request(CreditDuelloWalletRequest {
            public_id: "member-a1b2".into(),
            amount_minor: 0,
            reason: "Prime".into(),
            reference: "ref-123".into(),
        })
        .is_err());
    }

    #[test]
    fn sanitizes_wallets_without_relaying_stripe_account_ids() {
        let parsed = parse_wallets(&json!({
            "wallets": [{
                "publicId": "member-a1b2",
                "referralCode": "member-a1b2",
                "displayName": "Amina Martin",
                "email": "amina@example.test",
                "clickCount": 1_250,
                "creditedClickCount": 1_000,
                "availableMinor": 2_500,
                "currency": "eur",
                "canWithdraw": true,
                "connection": {
                    "status": "ready",
                    "payoutsEnabled": true,
                    "stripeAccountId": "acct_secret"
                }
            }]
        }))
        .unwrap();
        assert_eq!(parsed[0].currency, "EUR");
        assert_eq!(parsed[0].stripe_status.as_deref(), Some("ready"));
        assert_eq!(parsed[0].solana_address, None);
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(!serialized.contains("acct_secret"));
        assert!(!serialized.contains("stripeAccountId"));
    }

    #[test]
    fn relays_valid_solana_address_and_rejects_invalid_ones() {
        let valid_address = "So11111111111111111111111111111111111111112";
        let parsed = parse_wallets(&json!({
            "wallets": [{
                "publicId": "member-a1b2",
                "referralCode": "member-a1b2",
                "displayName": "Amina Martin",
                "email": "amina@example.test",
                "clickCount": 1,
                "creditedClickCount": 0,
                "availableMinor": 100,
                "currency": "eur",
                "solanaAddress": valid_address
            }]
        }))
        .unwrap();
        assert_eq!(parsed[0].solana_address.as_deref(), Some(valid_address));
        assert!(serde_json::to_string(&parsed)
            .unwrap()
            .contains(valid_address));

        // Alphabet invalide (0, O, I, l absents du base58), longueur hors borne
        // ou base58 valide qui ne represente pas exactement 32 octets.
        assert!(parse_wallets(&json!({
            "wallets": [{
                "publicId": "member-a1b2",
                "referralCode": "member-a1b2",
                "displayName": "Amina",
                "email": "amina@example.test",
                "clickCount": 1,
                "creditedClickCount": 0,
                "availableMinor": 1,
                "currency": "eur",
                "solanaAddress": "0OIl-not-base58"
            }]
        }))
        .is_err());
        assert!(parse_wallets(&json!({
            "wallets": [{
                "publicId": "member-a1b2",
                "referralCode": "member-a1b2",
                "displayName": "Amina",
                "email": "amina@example.test",
                "clickCount": 1,
                "creditedClickCount": 0,
                "availableMinor": 1,
                "currency": "eur",
                "solanaAddress": "22222222222222222222222222222222"
            }]
        }))
        .is_err());
        assert!(parse_wallets(&json!({
            "wallets": [{
                "publicId": "member-a1b2",
                "referralCode": "member-a1b2",
                "displayName": "Amina",
                "email": "amina@example.test",
                "clickCount": 1,
                "creditedClickCount": 0,
                "availableMinor": 1,
                "currency": "eur",
                "solanaAddress": "abc"
            }]
        }))
        .is_err());
    }
}
