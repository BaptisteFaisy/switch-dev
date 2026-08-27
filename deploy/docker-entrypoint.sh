#!/bin/sh
set -eu

install -d -m 0700 -o cst -g cst /srv/cst /srv/cst/codex-homes /srv/cst/workspaces
chmod 0700 /srv/cst /srv/cst/codex-homes /srv/cst/workspaces

# Corrige a cout constant par workspace les anciens deploiements Ansible qui
# recreaient leur racine en root:root. Sans le droit de parcours sur cette
# racine, le lancement de Codex echoue avec EACCES avant meme son demarrage.
find /srv/cst/workspaces -mindepth 1 -maxdepth 1 -type d -exec chown cst:cst {} +

# Les archives de seed sont extraites par Ansible en root avant le premier
# demarrage. Une seule correction recursive remet les comptes et le workspace
# au compte non privilegie du conteneur ; les redemarrages suivants restent O(1).
ownership_marker=/srv/cst/.container-ownership-v1
if [ ! -e "$ownership_marker" ]; then
  chown -R cst:cst /srv/cst
  chmod -R go-rwx /srv/cst
  : > "$ownership_marker"
  chown cst:cst "$ownership_marker"
  chmod 0600 "$ownership_marker"
fi

# ---------------------------------------------------------------------------
# Serveur SSH du conteneur : acces bidirectionnel poste <-> conteneur.
# Le pont est demarre automatiquement a chaque lancement du conteneur.
#
# Direction poste -> conteneur : sshd demarre ci-dessous (cles publiques
# uniquement), port 22 dans le conteneur, publie par compose sur
# 127.0.0.1:${CST_SSH_PORT:-2224} de l'hote. La cle hote et les authorized_keys
# vivent dans le volume persistant /srv/cst/ssh : elles survivent aux rebuilds
# d'image, la connexion ne change jamais d'empreinte.
#
# Direction conteneur -> poste : le poste (Windows) ecoute deja en SSH.
# scripts/setup-container-ssh.ps1 genere la paire id_back dans ce meme volume
# et ajoute sa partie publique aux authorized_keys du poste ; /home/cst/.ssh/
# config preconfigure « ssh pc-fixe » et conserve « ssh local » comme alias
# historique (host.docker.internal, compose extra_hosts).
# ---------------------------------------------------------------------------
ssh_dir=/srv/cst/ssh
install -d -m 0700 -o cst -g cst "$ssh_dir"
if [ ! -s "$ssh_dir/ssh_host_ed25519_key" ]; then
  ssh-keygen -q -t ed25519 -N "" -f "$ssh_dir/ssh_host_ed25519_key"
fi
chown cst:cst "$ssh_dir/ssh_host_ed25519_key" "$ssh_dir/ssh_host_ed25519_key.pub" 2>/dev/null || true
chmod 0600 "$ssh_dir/ssh_host_ed25519_key"
if [ ! -e "$ssh_dir/authorized_keys" ]; then
  : > "$ssh_dir/authorized_keys"
fi
chown cst:cst "$ssh_dir/authorized_keys"
chmod 0600 "$ssh_dir/authorized_keys"
if [ -s "$ssh_dir/id_back" ]; then
  chown cst:cst "$ssh_dir/id_back" "$ssh_dir/id_back.pub" 2>/dev/null || true
  chmod 0600 "$ssh_dir/id_back"
fi
# La clé retour peut avoir été préparée après le premier démarrage ; le pont
# conteneur -> PC reste disponible dès qu'elle existe dans le volume.


# Port du sshd du conteneur. En mode compose (port publie 2224:22) il reste
# sur 22 ; en reseau host, 22 appartient deja au sshd de la distro WSL, donc
# CST_SSHD_PORT doit pointer ailleurs (ex. 2224) pour que sshd demarre.
sshd_port=${CST_SSHD_PORT:-22}
cat > /etc/ssh/sshd_config <<EOF
Port $sshd_port
ListenAddress 0.0.0.0
HostKey $ssh_dir/ssh_host_ed25519_key
PermitRootLogin no
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
AllowUsers cst
AuthorizedKeysFile $ssh_dir/authorized_keys
StrictModes yes
Subsystem sftp /usr/lib/openssh/sftp-server
EOF
install -d -m 0755 /run/sshd
/usr/sbin/sshd || echo "cst-container: echec du demarrage sshd (le runtime continue)" >&2

# Sortie vers le poste hote : `ssh pc-fixe` depuis le conteneur.
local_user=${CST_SSH_LOCAL_USER:-jeanp}
local_port=${CST_SSH_LOCAL_PORT:-22}
# En reseau host, host.docker.internal pointe parfois vers une ancienne IP LAN
# (entree /etc/hosts obsolete). CST_SSH_LOCAL_HOST permet de forcer la cible
# reelle du poste (ex. IP Tailscale, seule route ouverte a travers le pare-feu
# Windows).
local_host=${CST_SSH_LOCAL_HOST:-host.docker.internal}
install -d -m 0700 -o cst -g cst /home/cst/.ssh
cat > /home/cst/.ssh/config <<EOF
# Pont SSH retour vers le PC. En production Azure, host.docker.internal est
# l'extremite du reverse-forward Tailscale porte par l'hote VPS.
Host local pc pc-fixe pc-fixe-tailscale
  HostName $local_host
  Port $local_port
  User $local_user
  IdentityFile $ssh_dir/id_back
  IdentitiesOnly yes
  StrictHostKeyChecking accept-new
  # Une seule connexion de controle persistante : `ssh pc-fixe` s'y rattache
  # instantanement au lieu de rouvrir une session, et autossh la maintient.
  ControlMaster auto
  ControlPath /home/cst/.ssh/ctl-%r@%h:%p
  ControlPersist 60
  ServerAliveInterval 30
  ServerAliveCountMax 3
  ConnectTimeout 10
EOF
chown -R cst:cst /home/cst/.ssh
chmod 0600 /home/cst/.ssh/config

# Pont persistant conteneur -> poste. autossh tient la connexion de controle
# ouverte en permanence et la retablit automatiquement apres une coupure du
# reseau ou un redemarrage du poste. Sans la cle id_back (setup jamais execute),
# le pont reste silencieusement absent, comme le sens sortant sur demande.
if [ -s "$ssh_dir/id_back" ]; then
  install -d -o cst -g cst /srv/cst/logs
  AUTOSSH_GATETIME=0 AUTOSSH_LOGLEVEL=1 /usr/sbin/gosu cst \
    autossh -M 0 -N \
      -o "ExitOnForwardFailure=yes" \
      local >> /srv/cst/logs/ssh-bridge.log 2>&1 &
  echo "cst-container: pont SSH conteneur -> poste lance (autossh)"
fi

# Le drain automatique est desactive par defaut. Il ne peut etre reactive que
# volontairement avec CST_MEMORY_WATCHDOG_ENABLED=1.
if [ "${CST_MEMORY_WATCHDOG_ENABLED:-0}" = "1" ]; then
  /usr/local/bin/cst-memory-watchdog &
fi

exec /usr/sbin/gosu cst "$@"
