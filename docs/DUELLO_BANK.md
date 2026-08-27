# Banque Duello — Switch développement

Cette vue appartient uniquement à Switch développement (SSD / pc-fixe). Elle ne
modifie, ne déploie et ne redémarre pas l'instance Switch stable sur Azure.

## Ce que fait la vue

La Banque Duello présente les portefeuilles affiliés fournis par Duello et permet
à un administrateur Switch de demander un crédit manuel du grand livre Duello.
Le crédit devient alors visible dans `https://duello.fr/dashboard` et peut être
retiré par le flux Stripe Connect déjà géré par Duello.

Deux opérations financières restent volontairement séparées :

1. **Crédit Duello** : écriture comptable dans le grand livre affilié Duello.
2. **Approvisionnement Stripe** : argent réel disponible sur le solde de la
   plateforme Stripe. La vue ouvre le Dashboard Stripe, mais Switch ne possède
   aucune clé secrète Stripe et ne considère jamais un retour navigateur comme
   une preuve de paiement.

Un crédit Duello n'approvisionne donc pas automatiquement Stripe. L'opérateur
doit vérifier le solde de plateforme avant de créditer un portefeuille.

## Contrat serveur attendu

Switch n'appelle jamais Duello directement depuis le navigateur. Son serveur
utilise un jeton dédié pour :

- `GET /api/admin/affiliate-wallets` ;
- `POST /api/admin/affiliate-credits`.

Le second endpoint doit être confirmé sur l'API Duello ciblée avant activation.
Il doit accepter `publicId`, `amountMinor`, `clickCountSnapshot`, `linkSlug`,
`reason` et `reference`, puis garantir l'idempotence de la référence. Les sources
Duello C: examinées contiennent un ancien proxy pour ce contrat, mais pas de flux
Stripe d'approvisionnement vérifié de bout en bout. Tant que le contrat privé
n'est pas confirmé, laisser la Banque Duello non configurée.

Pour un paiement entrant automatisé, le contrat Duello devra créer une session
Stripe côté serveur et créditer le grand livre uniquement après validation d'un
webhook Stripe signé et dédupliqué. Le succès d'une redirection Checkout ne doit
jamais déclencher un crédit.

## Configuration Switch

Les variables sont optionnelles et l'interface affiche un état sûr « non
configuré » lorsqu'elles manquent :

```dotenv
CST_DUELLO_BANK_API_ORIGIN=https://duello-api.internal.example
CST_DUELLO_BANK_ADMIN_TOKEN=replace-with-a-long-random-duello-admin-token
CST_DUELLO_DASHBOARD_URL=https://duello.fr/dashboard
```

L'origine API doit être HTTPS, sauf HTTP sur une adresse de boucle locale. Elle
ne peut contenir ni chemin, ni paramètres, ni identifiants. Le jeton est conservé
dans la configuration serveur puis retiré de l'environnement avant la création
des terminaux et providers enfants.

## Garde-fous

- routes Switch réservées au jeton `CST_ADMIN_TOKEN` exact ; une session
  utilisateur ordinaire ne suffit pas ;
- origine navigateur contrôlée pour la mutation ;
- corps JSON limité à 8 Kio et champs inconnus refusés ;
- montants EUR entiers de 1 à 10 000 000 centimes ;
- données du portefeuille relues côté serveur avant chaque crédit ;
- clé d'idempotence SHA-256 stable dérivée de la référence ;
- réponses Duello bornées, redirections HTTP refusées et aucune donnée de compte
  Stripe relayée au navigateur ;
- réponses financières marquées `Cache-Control: no-store`.
