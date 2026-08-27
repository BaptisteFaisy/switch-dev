import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const bashProvisioner = readFileSync(
  new URL("../scripts/provision-oracle-free.sh", import.meta.url),
  "utf8",
);
const powershellProvisioner = readFileSync(
  new URL("../scripts/provision-oracle-free.ps1", import.meta.url),
  "utf8",
);
const retryProvisioner = readFileSync(
  new URL("../scripts/retry-oracle-provision.ps1", import.meta.url),
  "utf8",
);
const bootstrapProfile = readFileSync(
  new URL("../scripts/bootstrap-oci-profile.sh", import.meta.url),
  "utf8",
);
const capacityCheck = readFileSync(
  new URL("../scripts/check-oracle-a1-capacity.sh", import.meta.url),
  "utf8",
);
const packageJson = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));

test("le provisionneur Oracle reste en lecture seule sans -Apply", () => {
  assert.match(bashProvisioner, /APPLY=0/);
  assert.match(bashProvisioner, /if \(\(APPLY == 0\)\); then/);
  assert.ok(
    bashProvisioner.indexOf("if ((APPLY == 0)); then") <
      bashProvisioner.indexOf("oci iam compartment create"),
    "le retour plan-only doit preceder la premiere mutation OCI",
  );
  assert.match(powershellProvisioner, /\[switch\]\$Apply/);
  assert.match(powershellProvisioner, /-Deploy requiert -Apply/);
});

test("les plafonds Always Free conservateurs sont imposes avant creation", () => {
  assert.match(bashProvisioner, /OCPUS="2"/);
  assert.match(bashProvisioner, /MEMORY_GB="12"/);
  assert.match(bashProvisioner, /BOOT_VOLUME_GB="50"/);
  assert.match(bashProvisioner, /used_cpu \+ add_cpu > 2/);
  assert.match(bashProvisioner, /used_mem \+ add_mem > 12/);
  assert.match(bashProvisioner, /used_disk \+ add_disk > 200/);
  assert.match(bashProvisioner, /CONFIG_REGION.*HOME_REGION/);
  assert.match(bashProvisioner, /VM\.Standard\.A1\.Flex/);
});

test("le reseau Oracle ne publie que SSH depuis le CIDR autorise", () => {
  assert.match(bashProvisioner, /destinationPortRange.*min.*22.*max.*22/);
  assert.match(bashProvisioner, /source.*ALLOWED_SSH_CIDR/);
  assert.match(bashProvisioner, /--ingress-security-rules '\[\]'/);
  assert.doesNotMatch(bashProvisioner, /destinationPortRange.*8080/);
  assert.doesNotMatch(bashProvisioner, /CST_BIND=.*0\.0\.0\.0/);
  assert.match(bashProvisioner, /Le service CST restera lie a 127\.0\.0\.1/);
});

test("le wrapper enchaine la VM validee avec le deploiement SSH CST", () => {
  assert.match(powershellProvisioner, /Get-CurrentPublicIPv4/);
  assert.match(powershellProvisioner, /Wait-Ssh -HostName \$publicIp/);
  assert.match(powershellProvisioner, /Wait-CloudInit -HostName \$publicIp/);
  assert.match(powershellProvisioner, /cloud-init status --wait/);
  assert.match(powershellProvisioner, /"deploy-vps-ansible\.ps1"/);
  assert.match(powershellProvisioner, /SshTarget = "ubuntu@\$publicIp"/);
  assert.match(powershellProvisioner, /AcceptNewHostKey = \$true/);
  assert.match(packageJson.scripts["provision:oracle"], /provision-oracle-free\.ps1/);
});

test("une VM geree existante est validee et ne peut pas etre dupliquee silencieusement", () => {
  assert.match(bashProvisioner, /existing-instance\.json/);
  assert.match(bashProvisioner, /item\.get\("shape"\) != expected_shape/);
  assert.match(bashProvisioner, /actual_cpu != expected_cpu/);
  assert.match(bashProvisioner, /actual_memory != expected_memory/);
  assert.match(bashProvisioner, /EXISTING_INSTANCE_STATE.*STOPPED/);
  assert.match(bashProvisioner, /--action START/);
});

test("les listes vides d'un compte OCI neuf sont normalisees", () => {
  assert.match(bashProvisioner, /ensure_list_json\(\)/);
  assert.match(bashProvisioner, /'\{"data":\[\]\}'/);
  assert.match(bashProvisioner, /ensure_list_json "\$TMP_DIR\/instances\/\$index\.json"/);
  assert.match(bashProvisioner, /ensure_list_json "\$TMP_DIR\/vcns\.json"/);
  assert.match(bashProvisioner, /ensure_list_json "\$TMP_DIR\/nsg-rules\.json"/);
});

test("le lanceur A1 ne cree que dans un fault domain annonce disponible", () => {
  assert.match(bashProvisioner, /oci iam fault-domain list/);
  assert.match(bashProvisioner, /compute compute-capacity-report create/);
  assert.match(bashProvisioner, /availability-status.*AVAILABLE/);
  assert.match(bashProvisioner, /for fault_domain in "\$\{AVAILABLE_FAULT_DOMAINS\[@\]\}"/);
  assert.match(bashProvisioner, /fault_launch_args\+=\(--fault-domain "\$fault_domain"\)/);
  assert.match(bashProvisioner, /laisser OCI choisir le meilleur placement/);
  assert.match(bashProvisioner, /break 3/);
  assert.match(bashProvisioner, /TooManyRequests\|too many requests/);
});

test("le retry Oracle est borne, exclusif et conserve le mode sans seed", () => {
  assert.match(retryProvisioner, /\[int\]\$MaxAttempts = 72/);
  assert.match(retryProvisioner, /Local\\CSTOracleAlwaysFreeProvision/);
  assert.match(retryProvisioner, /if \(-not \$IncludeAccountSeed\).*SkipAccountSeed/);
  assert.match(retryProvisioner, /Ocpus = 2; MemoryGB = 12; Capacity = 2/);
  assert.match(retryProvisioner, /Ocpus = 1\s+MemoryGB = 6\s+Capacity = 1/);
  assert.match(retryProvisioner, /"--ocpus", \[string\]\$capacityProfile\.Ocpus,/);
  assert.match(retryProvisioner, /Ocpus = \[int\]\$availableProfile\.Ocpus/);
  assert.match(retryProvisioner, /\[switch\]\$ForceLaunch/);
  assert.match(retryProvisioner, /\[switch\]\$FullSizeOnly/);
  assert.match(retryProvisioner, /tentative reelle forcee/);
  assert.match(retryProvisioner, /Wait-InChunks -Seconds \$DelaySeconds/);
  assert.match(retryProvisioner, /provision-success\.json/);
  assert.match(retryProvisioner, /Provisionnement Oracle deja termine; controle ignore/);
  assert.match(retryProvisioner, /check-oracle-a1-capacity\.sh/);
  assert.match(retryProvisioner, /\$capacityExit -eq 75/);
});

test("le controle leger de capacite ne lance aucune instance", () => {
  assert.match(capacityCheck, /compute compute-capacity-report create/);
  assert.match(capacityCheck, /A1_CAPACITY_AVAILABLE=false/);
  assert.match(capacityCheck, /exit 75/);
  assert.doesNotMatch(capacityCheck, /compute instance launch/);
  assert.doesNotMatch(capacityCheck, /network .* create/);
});

test("le nettoyage du bootstrap reste confine au repertoire de session OCI", () => {
  assert.match(bootstrapProfile, /--cleanup-bootstrap/);
  assert.match(bootstrapProfile, /os\.path\.commonpath\(\(session_root, candidate\)\)/);
  assert.match(bootstrapProfile, /os\.path\.basename\(session_dir\) != profile/);
  assert.match(bootstrapProfile, /parser\.remove_section\(profile\)/);
  assert.match(bootstrapProfile, /BOOTSTRAP_SESSION_REMOVED=true/);
  assert.doesNotMatch(bootstrapProfile, /rmtree/);
});
