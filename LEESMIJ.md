# AudioShare Uploader 0.3.0

Uploadt programma's automatisch naar AudioShare vanaf een server-pc in de studio: op een vaste tijd, of zodra een bestand vernieuwd is. Draait op de achtergrond onder de ingelogde Windows-gebruiker (zoals FileZilla of RoboTask) en wordt ingesteld via een webinterface.

## Installeren (per studio)

1. In AudioShare (admin): **Recorders → Studio's**. Maak een studio aan (bijv. "Server Studio 2"), vink de programma's aan en klik op **Uploader downloaden**.
2. Pak de zip uit op de server-pc in een vaste map, bijv. `C:\AudioShareUploader\`.
3. Log in op Windows met de gebruiker die bij de audiobestanden kan (dezelfde schijven, zoals `Y:`).
4. **Dubbelklik** op `AudioShareUploader.exe`. De browser opent `http://localhost:8470`.
5. Kies een gebruikersnaam en wachtwoord (minimaal 10 tekens). De uploader heeft zichzelf al gekoppeld met de studio.
6. **Taken → Taak toevoegen.**

Geen beheerrechten nodig. Voortaan start de uploader vanzelf als deze Windows-gebruiker inlogt; zorg dat de pc na een herstart automatisch inlogt. Nog een keer dubbelklikken opent alleen de webinterface.

Vanaf andere pc's in het netwerk: `http://<naam-van-de-server>:8470` (sta poort 8470 toe in de Windows Firewall).

**Van een eerdere service-installatie (0.1/0.2)?** Eerst in een opdrachtprompt als administrator: `AudioShareUploader.exe uninstall`. Daarna dubbelklikken. Taken blijven bewaard; koppel opnieuw met een nieuwe code (Instellingen), omdat de sleutel bij het service-account hoorde.

## Een taak

| Instelling | Uitleg |
|---|---|
| Programma | Uit de lijst die AudioShare voor deze studio toestaat |
| Bron | Een vast bestand, of een map (dan het nieuwste audiobestand: MP3, WAV, FLAC, OGG). Netwerkpaden (`\\nas\audio\...`) kunnen, mits de service er toegang toe heeft (zie hieronder) |
| Moment | **Bewaken**: uploaden zodra het bestand nieuw of veranderd is. **Vaste tijd**: op gekozen dagen en tijdstip |
| Wachttijd | Een bestand moet zo lang ongewijzigd zijn (standaard 60 s), zodat een half weggeschreven opname niet meegaat |
| Uitzenddatum | Vandaag, morgen, eerstvolgende uitzenddag (vandaag telt mee), of uit de bestandsnaam (`2026-11-14`, `20261114`, `14-11-2026`) |
| Titel | Sjabloon met `{programma}`, `{datum}`, `{dag}`, `{bestand}` |

- Bij een nieuwe taak (of een andere bron) geldt het bestand dat er nu staat als al verstuurd. Wil je het toch uploaden: **Nu uploaden**.
- Staat er al een aflevering op die datum, dan wordt die vervangen.
- Een bestand met dezelfde inhoud wordt niet nog eens voor dezelfde datum geüpload.
- Voldoet een bestand niet aan de normen (WAV, FLAC, OGG, of een MP3 met lagere bitrate), dan zet de Uploader het om naar MP3 320 kbps. Het origineel wordt nooit veranderd, verplaatst of verwijderd.
- Een geplande upload die meer dan 6 uur gemist is (server uit), wordt overgeslagen en in het logboek gezet.
- Bij elke aflevering staat in AudioShare "bediend door: Automatische upload (taaknaam)".

## Meldingen

De Uploader meldt problemen aan AudioShare (vanaf versie 1.6.0) en AudioShare mailt ze naar het meldingsadres uit de platforminstellingen: mislukte upload, een uur lang geen verbinding, geen nieuw bestand op een vaste tijd, en een bron die niet gevonden wordt (max. 1 keer per dag). Alles staat ook in het **Logboek**.

## Beveiliging

- Koppeling als **studio**: de sleutel geeft alleen toegang tot de programma's van die studio en is in AudioShare met één klik in te trekken. Op de server staat hij in Windows Credential Manager van de ingelogde gebruiker (versleuteld door Windows). Het koppelbestand in de zip werkt één keer en krijgt daarna de extensie `.gebruikt`.
- Webinterface: inloggen met gebruikersnaam en wachtwoord (Argon2id), sessie van 12 uur, na 5 foute pogingen 5 minuten wachten. Het eerste account kan alleen op de server zelf worden gemaakt.
- Standaard alleen bereikbaar vanaf het lokale netwerk. Zet toegang van buiten pas aan als het via een VPN of een beveiligde (HTTPS-)proxy loopt; de webinterface zelf spreekt gewoon HTTP.

## Netwerkmappen (NAS)

Normaal niet nodig: de uploader ziet dezelfde schijven als de ingelogde gebruiker. Alleen als hij een netwerkmap toch niet kan lezen (of als hij als service draait): **Instellingen → Netwerkmappen**, met schijfletter, netwerkpad (`\\NAS\share`) en een gebruiker en wachtwoord van de NAS.

## Bijwerken naar een nieuwe versie

1. Webinterface → Instellingen → **Uploader afsluiten**.
2. Vervang `AudioShareUploader.exe` door de nieuwe versie.
3. Dubbelklik erop.

Taken, koppeling en instellingen blijven bewaard.

## Beheer

- Gegevens: `C:\ProgramData\AudioShareUploader\` (`settings.json`, `tasks.json`, `uploads.json` met wachtrij en logboek, `werk\` voor omgezette kopieën).
- Testen in een venster: `AudioShareUploader.exe run`.
- Niet meer automatisch starten: Instellingen → vinkje uit. Afsluiten: Instellingen → Uploader afsluiten.
- Als service (alleen als de pc niet ingelogd blijft): `AudioShareUploader.exe install` / `uninstall` als administrator; zie dan ook Netwerkmappen.
- Een omzetting gebeurt één tegelijk; een lange WAV omzetten duurt even, andere taken wachten zo lang.

## Techniek

Rust. Upload-, omzet- en beveiligingscode gedeeld met de Studio Recorder (`src/audioshare`, `decode.rs`, `export.rs`, `wav.rs`). `rules.rs`: bron, datum, titel en planning (met tests). `engine.rs`: de lus. `web.rs` + `web/`: de webinterface. Bouwen voor Windows: `build-win.sh` (cross-compile) of `cargo build --release` op Windows.
