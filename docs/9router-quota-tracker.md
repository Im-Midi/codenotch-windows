# 9Router Quota Tracker in Codenotch

Codenotch can show everything [9Router](https://github.com/decolua/9router)'s **Quota Tracker** page shows — every connected account (Antigravity, Claude, Codex, Kiro, …), how much of each quota is left, and when it resets — in the notch, without opening the dashboard.

*Bahasa Indonesia: [lihat di bawah](#bahasa-indonesia).*

---

## What you get

| Where | What it shows |
|---|---|
| **9r ring** | The single quota closest to running out across all accounts, as the share **used** (like every other ring). |
| **Card** (hover the 9r cell) | A block per account: each quota's share **left**, a bar, and the reset countdown — the same rows as 9Router's Quota Tracker. Hover a row for the raw numbers (`35 / 1,000`); scroll when there are many accounts. |
| **Note** under the card | Requests and cost today, from 9Router's Usage page. |

Quotas you hide with the eye icon in 9Router's Quota Tracker stay hidden in Codenotch.

---

## 1. 9Router on the same PC

1. Install and start 9Router:
   ```bash
   npm install -g 9router
   9router
   ```
2. Open the dashboard at `http://localhost:20128` and **sign in once** (default password `123456` — change it). That first sign-in creates the two files Codenotch derives its token from (`%APPDATA%\9router\machine-id` and `auth\cli-secret`).
3. Connect your accounts in 9Router (**Providers**).
4. In Codenotch: tray → **Refresh usage now**. The **9r** cell appears. Nothing needs typing.

## 2. 9Router on another computer or server

Open Codenotch → tray → **API keys…** → **9Router** and fill in:

- **Server URL** — the dashboard's address, e.g. `https://9router.example.com`. The `/v1` ending shown under *API Endpoint* in 9Router is removed automatically.
- **CLI token** — 16 hex characters, computed on the machine running 9Router. **Not** one of the `sk-…` keys listed under *API Keys*: those only work for the LLM proxy, and 9Router's usage API refuses them.

Get the CLI token by running this on the 9Router machine (inside the container for Docker):

**Linux / macOS / Docker** (on macOS replace `sha256sum` with `shasum -a 256`):
```bash
D=${DATA_DIR:-$HOME/.9router}; printf '%s9r-cli-auth%s' "$(tr -d '[:space:]' < "$D/machine-id")" "$(tr -d '[:space:]' < "$D/auth/cli-secret")" | sha256sum | cut -c1-16
```

**Windows (PowerShell)** — or install Codenotch there and click **Copy this PC's token**:
```powershell
$d="$env:APPDATA\9router"; $s=(Get-Content "$d\machine-id" -Raw).Trim()+"9r-cli-auth"+(Get-Content "$d\auth\cli-secret" -Raw).Trim(); -join ([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($s))|%{$_.ToString('x2')}) | %{$_.Substring(0,16)}
```

Press **Save & test**. `Connected to … · N requests today` means it works.

> Why a CLI token? 9Router's dashboard API (`/api/usage/…`) accepts only its own CLI token (`sha256(machine-id + "9r-cli-auth" + cli-secret)`, first 16 hex characters) or a signed-in browser session. It is the same token 9Router's own CLI uses.

## 3. 9Router behind Cloudflare Access

If opening the URL shows **"Sign in · Cloudflare Access"**, every request is stopped before it reaches 9Router. Give Codenotch a service token:

1. Cloudflare Zero Trust → **Access controls → Service credentials → Service Tokens** → **Create Service Token**. Copy the Client Secret right away — Cloudflare shows it only once.
2. Zero Trust → **Access controls → Applications** → your 9Router application → **Policies** → **Add a policy**:
   - Action: **Service Auth** (an *Allow* policy still demands a login)
   - Include: **Service Token** → the token from step 1

   The policy must be **attached to the application** — one that only exists on the *Policies* page does nothing.
3. In Codenotch → **API keys…** → **Behind Cloudflare Access?** paste the Client ID and Client Secret (the whole `CF-Access-Client-Id: …` line from Cloudflare's copy button is fine) → **Save & test**.

## How often it refreshes

Each account read makes 9Router query that vendor's quota API, so Codenotch stays gentle: accounts every **5 minutes**, Claude every **10** (9Router's own dashboard throttles Claude the same way). The request count updates every 2 minutes. Tray → **Refresh usage now** forces a full read.

## Troubleshooting

| Message | Fix |
|---|---|
| *This URL is behind Cloudflare Access…* | Add a service token (section 3). |
| *Cloudflare Access refused the service token* | The Service Auth policy is missing, is an *Allow* policy, or isn't attached to the 9Router application. |
| *…the saved token is one of its proxy API keys (sk-…)* | You pasted an API key. Use the CLI token (section 2). |
| *Reached 9Router, but it rejected this CLI token* | The token is from another install, or `auth/cli-secret` changed. Run the command again. |
| *…answered with a web page* / *…redirects to…* | The URL isn't the dashboard address. Check it opens the dashboard in a browser. |
| An account says *…needs re-authorising in 9Router* | That provider's sign-in expired inside 9Router — reconnect it there. |
| No 9r cell at all | 9Router isn't installed here and nothing is saved in **API keys…** — see sections 1–2. |

## Security

- The CLI token opens 9Router's **admin API** (settings, shutdown, updates). Treat it like a password and don't share screenshots of it.
- To replace it: delete `auth/cli-secret` in 9Router's data folder, restart 9Router, run the command again.
- Everything entered in **API keys…** is stored in Windows Credential Manager, never in a plain file. Account tokens stay inside 9Router: the account list Codenotch reads is an allowlist without them.

---

## Bahasa Indonesia

Codenotch bisa menampilkan semua yang ada di halaman **Quota Tracker** [9Router](https://github.com/decolua/9router) — semua akun yang tersambung (Antigravity, Claude, Codex, Kiro, …), sisa tiap kuota, dan kapan reset — langsung di notch, tanpa membuka dashboard.

### Yang ditampilkan

| Tempat | Isinya |
|---|---|
| **Ring 9r** | Satu kuota yang paling dekat habis dari semua akun, dalam persen **terpakai** (sama seperti ring lain). |
| **Card** (arahkan kursor ke sel 9r) | Satu blok per akun: persen **sisa** tiap kuota, bar, dan hitung mundur reset — sama dengan Quota Tracker 9Router. Arahkan kursor ke baris untuk angka mentahnya (`35 / 1,000`); scroll kalau akunnya banyak. |
| **Catatan** di bawah card | Jumlah request dan biaya hari ini, dari halaman Usage 9Router. |

Kuota yang kamu sembunyikan dengan ikon mata di Quota Tracker 9Router tetap tersembunyi di Codenotch.

### 1. 9Router di PC yang sama

1. Pasang dan jalankan 9Router:
   ```bash
   npm install -g 9router
   9router
   ```
2. Buka dashboard di `http://localhost:20128` dan **login sekali** (password bawaan `123456` — segera ganti). Login pertama itu membuat dua file yang dipakai Codenotch untuk menghitung token-nya (`%APPDATA%\9router\machine-id` dan `auth\cli-secret`).
3. Sambungkan akun-akunmu di 9Router (**Providers**).
4. Di Codenotch: tray → **Refresh usage now**. Sel **9r** akan muncul. Tidak perlu mengetik apa pun.

### 2. 9Router di komputer lain atau server

Buka Codenotch → tray → **API keys…** → **9Router**, lalu isi:

- **Server URL** — alamat dashboard, misalnya `https://9router.example.com`. Akhiran `/v1` yang tampil di bagian *API Endpoint* 9Router dibuang otomatis.
- **CLI token** — 16 karakter hex, dihitung di mesin yang menjalankan 9Router. **Bukan** key `sk-…` di daftar *API Keys*: key itu hanya untuk proxy LLM, dan API usage 9Router menolaknya.

Ambil CLI token dengan menjalankan perintah ini di mesin 9Router (di dalam container kalau pakai Docker):

**Linux / macOS / Docker** (di macOS ganti `sha256sum` dengan `shasum -a 256`):
```bash
D=${DATA_DIR:-$HOME/.9router}; printf '%s9r-cli-auth%s' "$(tr -d '[:space:]' < "$D/machine-id")" "$(tr -d '[:space:]' < "$D/auth/cli-secret")" | sha256sum | cut -c1-16
```

**Windows (PowerShell)** — atau pasang Codenotch di sana lalu klik **Copy this PC's token**:
```powershell
$d="$env:APPDATA\9router"; $s=(Get-Content "$d\machine-id" -Raw).Trim()+"9r-cli-auth"+(Get-Content "$d\auth\cli-secret" -Raw).Trim(); -join ([Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($s))|%{$_.ToString('x2')}) | %{$_.Substring(0,16)}
```

Tekan **Save & test**. Kalau muncul `Connected to … · N requests today`, berarti berhasil.

### 3. 9Router di balik Cloudflare Access

Kalau membuka URL-nya memunculkan **"Sign in · Cloudflare Access"**, semua request tertahan sebelum sampai ke 9Router. Beri Codenotch sebuah service token:

1. Cloudflare Zero Trust → **Access controls → Service credentials → Service Tokens** → **Create Service Token**. Salin Client Secret-nya saat itu juga — Cloudflare hanya menampilkannya sekali.
2. Zero Trust → **Access controls → Applications** → aplikasi 9Router-mu → **Policies** → **Add a policy**:
   - Action: **Service Auth** (policy *Allow* tetap meminta login)
   - Include: **Service Token** → token dari langkah 1

   Policy-nya harus **terpasang di aplikasi** — policy yang hanya ada di halaman *Policies* tidak berpengaruh apa-apa.
3. Di Codenotch → **API keys…** → **Behind Cloudflare Access?** tempel Client ID dan Client Secret (baris utuh `CF-Access-Client-Id: …` dari tombol copy Cloudflare boleh langsung ditempel) → **Save & test**.

### Seberapa sering refresh

Setiap pembacaan akun membuat 9Router menghubungi API kuota vendornya, jadi Codenotch sengaja pelan: akun tiap **5 menit**, Claude tiap **10 menit** (dashboard 9Router sendiri juga membatasi Claude seperti itu). Jumlah request diperbarui tiap 2 menit. Tray → **Refresh usage now** memaksa pembacaan penuh.

### Kalau ada masalah

| Pesan | Solusi |
|---|---|
| *This URL is behind Cloudflare Access…* | Tambahkan service token (bagian 3). |
| *Cloudflare Access refused the service token* | Policy Service Auth belum ada, masih berupa policy *Allow*, atau belum terpasang di aplikasi 9Router. |
| *…the saved token is one of its proxy API keys (sk-…)* | Yang tertempel adalah API key. Pakai CLI token (bagian 2). |
| *Reached 9Router, but it rejected this CLI token* | Token dari instalasi lain, atau `auth/cli-secret` berubah. Jalankan perintahnya lagi. |
| *…answered with a web page* / *…redirects to…* | URL-nya bukan alamat dashboard. Pastikan URL itu membuka dashboard di browser. |
| Sebuah akun bertuliskan *…needs re-authorising in 9Router* | Login provider itu kedaluwarsa di dalam 9Router — sambungkan ulang di sana. |
| Sel 9r tidak muncul sama sekali | 9Router tidak terpasang di PC ini dan belum ada yang disimpan di **API keys…** — lihat bagian 1–2. |

### Keamanan

- CLI token membuka **API admin** 9Router (pengaturan, shutdown, update). Perlakukan seperti password, dan jangan membagikan screenshot-nya.
- Untuk menggantinya: hapus `auth/cli-secret` di folder data 9Router, restart 9Router, lalu jalankan perintahnya lagi.
- Semua yang dimasukkan di **API keys…** disimpan di Windows Credential Manager, tidak pernah di file biasa. Token akun tetap di dalam 9Router: daftar akun yang dibaca Codenotch adalah allowlist tanpa token.
