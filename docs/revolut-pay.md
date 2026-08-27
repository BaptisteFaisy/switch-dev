# Paiement via Revolut Pay

Codex Switch Terminal peut accepter un paiement via **Revolut Pay**, la solution de
paiement de Revolut qui laisse le client payer depuis son app Revolut (ou en CB
via le checkout Revolut), **sans que Switch ne manipule jamais une carte, un IBAN
ou un jeton de paiement**.

> Point important : Revolut Pay n'est pas un « login ». Revolut n'offre pas de
> bouton « Se connecter avec Revolut » comme Google. C'est un **moyen de
> paiement marchand** qui s'appuie sur la **Merchant API** de Revolut. L'argent
> transite exclusivement chez Revolut, jamais par l'application.

## Architecture visée

Revolut fournit deux briques que l'intégration réunit :

1. côté serveur : la **Merchant API** (créer une commande, vérifier son état,
   recevoir les webhooks de cycle de vie) ;
2. côté client : **RevolutCheckout.js**, qui monte le widget « Payer avec
   Revolut » et redirige le client vers Revolut pour l'autorisation.

Le flux :

- l'utilisateur demande un paiement depuis le dashboard (montant, devise, référence) ;
- Switch crée la commande chez Revolut via `POST /orders` et reçoit un `id` et un
  `public_id` ;
- le frontend monte RevolutCheckout.js avec ces identifiants ; le client
  finalise chez Revolut ;
- Revolut redirige le client vers l'`return_url` (appelée `redirect_url`), puis
  envoie un webhook `order.completed` / `transaction.*` ;
- Switch vérifie l'état de la commande chez Revolut (`GET /orders/{id}`) et
  journalise le reçu **sans** considérer un clic comme une preuve de débit.

## Prérequis Revolut (compte marchand)

Revolut Pay exige un **compte Revolut Business** avec le service marchand activé.
Les étapes se font sur le **portail Revolut Business**, pas dans Switch.

1. Créer (ou ouvrir) un compte [Revolut Business](https://www.revolut.com/business/).
2. Activer Revolut Pay et demander le **Merchant API** :
   **Paramètres → APIs → Merchant API**.
3. Générer les identifiants de la **Production API** :
   - le **public key / client id** (désigné `public_id`) : avertissement côté
     client, sert à monter RevolutCheckout.js ;
   - le **secret key** : jeton côté serveur, utilisé dans l'en-tête
     `Authorization` pour `POST /orders` et `GET /orders/{id}`.
4. Enregistrer les URL de callback et de webhook demandées par Revolut :
   - la **redirect / return URL** : `https://<votre-domaine>/api/revolut/return` ;
   - le **webhook** : `https://<votre-domaine>/api/revolut/webhook`, abonné aux
     événements `order.*` et `transaction.*`.
   Revolut rejette les URLs HTTP sauf en boucle locale de test ; production et
   sandbox ont des jeux d'identifiants et des domaines distincts.

Revolut propose un **sandbox** (`sandbox-merchant.revolut.com`) pour tester avant
la production. L'intégration Switch distingue toujours
`CST_REVOLUT_SANDBOX` (voir plus bas) afin de valider le flux avec les cartes de
test Revolut avant d'exposer de l'argent réel.

## Configuration dans Switch

L'intégration est **inactive tant que les identifiants ne sont pas présents**,
comme les autres connecteurs (Google, Microsoft, WhatsApp). Variables
d'environnement du serveur `cst-server` :

```bash
# Identifiants marchand Revolut (Production API).
CST_REVOLUT_PUBLIC_KEY="...public id / client id..."
CST_REVOLUT_SECRET_KEY="...secret key..."

# Devise ISO par defaut pour un paiement : FR -> EUR.
CST_REVOLUT_CURRENCY="EUR"

# Facultatif. "true" pour appeler l'environnement de test (sandbox) de Revolut.
# Production attendrait la valeur absente ou "false".
# CST_REVOLUT_SANDBOX="true"
# CST_REVOLUT_MERCHANT_BASE_URL="https://sandbox-merchant.revolut.com"

# Facultatifs si CST_PUBLIC_BASE_URL est correcte ; sinon les remplacer par
# vos URLs publiques exactes (Revolut compare caractere par caractere).
# CST_REVOLUT_RETURN_URL="https://<domaine>/api/revolut/return"
# CST_REVOLUT_WEBHOOK_URL="https://<domaine>/api/revolut/webhook"
```

`CST_PUBLIC_BASE_URL` doit pointer vers l'origine HTTPS publique du serveur ;
les redirect et webhook Revolut en sont déduits par défaut.

### Routes

Une fois la Merchant API configurée, Switch monte sous `/api/revolut` :

| Route                 | Rôle                                                              |
| --------------------- | ----------------------------------------------------------------- |
| `GET /status`         | état de la configuration (configurée ou non), jamais de secret     |
| `POST /orders`        | créer une commande Revolut, renvoie `id` + `public_id`             |
| `GET /orders/:id`     | état courant d'une commande (reçu / échec)                         |
| `GET /return`         | retour client après le checkout Revolut                           |
| `POST /webhook`       | événements `order.*` / `transaction.*` de Revolut                 |

Aucune carte, aucun IBAN et aucun secret ne sont renvoyés au navigateur : seul
le `public_id` transite côté client pour monter le widget.

## Dans le dashboard

Sur le VPS, une carte **Revolut Pay** apparaît dans les réglages de
l'application dès que `CST_REVOLUT_PUBLIC_KEY` et `CST_REVOLUT_SECRET_KEY` sont
définis et que le serveur est redémarré :

- elle affiche l'état (non configuré / connecté / sandbox) ;
- elle permet de lancer un paiement d'un montant et d'une devise, avec une
  référence libre ;
- le bouton **Payer avec Revolut** monte le checkout RevolutPay ; l'argent reste
  chez Revolut, Switch vérifie ensuite le statut du reçu.

## Sécurité

- le secret marchand reste côté serveur, uniquement sur l'hôte `cst-server` ;
- le paiement crée une commande chez Revolut ; le clic n'est jamais interprété
  comme un débit ; l'état réel est relu via la Merchant API et les webhooks ;
- chaque webhook est signé par Revolut et vérifié avant traitement ;
- la configuration est stockée hors des réponses API et du code frontend.

## Ce qu'il faut fournir pour finaliser le code

Dès que le compte marchand Revolut existe (production ou sandbox), fournis ces
quatre valeurs afin de câbler et tester l'intégration :

1. le **public key / client id** (public_id) ;
2. le **secret key** marchand ;
3. l'**URL de retour** validée (`/api/revolut/return`) ;
4. l'**URL de webhook** validée (`/api/revolut/webhook`), avec les abonnements
   d'événements choisis.

À partir de là, l'implémentation des routes est écrite, compilée et testée en
sandbox avant toute activation sur le VPS de production.