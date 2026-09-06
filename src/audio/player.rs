use anyhow::Result;
use librespot_core::authentication::Credentials;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot::playback::audio_backend;
use librespot::playback::config::{AudioFormat, PlayerConfig};
use librespot::playback::mixer::softmixer::SoftMixer;
use librespot::playback::mixer::{Mixer, MixerConfig};
use librespot::playback::player::{Player, PlayerEvent, PlayerEventChannel};
use librespot_protocol::authentication::AuthenticationType;
use log::{error, info};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[allow(dead_code)]
pub enum PlayerCommand {
    Load { uri: String, play: bool },
    Play,
    Pause,
    Toggle,
    Seek(u32),
    SetVolume(f32),
    Stop,
}

#[derive(Debug, Clone)]
pub enum PlayerEventMsg {
    Position(u32),
    StateChanged { is_playing: bool },
    EndOfTrack,
}

pub struct AudioPlayer {
    player: Player,
    player_events: PlayerEventChannel,
    #[allow(dead_code)]
    session: Session,
    mixer: Box<dyn Mixer>,
}

impl AudioPlayer {
    /// Conecta usando credenciais fornecidas (seja token OAuth ou senha)
    pub async fn new_with_credentials(credentials: Credentials) -> Result<Self> {
        let session_config = SessionConfig {
            ap_port: Some(443),
            ..Default::default()
        };

        info!("Conectando ao Spotify Connect via librespot (320kbps)...");
        let (session, _) = Session::connect(session_config, credentials, None, true)
            .await
            .map_err(|e| anyhow::anyhow!("Falha ao conectar no Spotify: {:?}", e))?;

        info!("Sessão librespot conectada com sucesso!");

        let player_config = PlayerConfig {
            bitrate: librespot::playback::config::Bitrate::Bitrate320,
            ..Default::default()
        };

        let audio_format = AudioFormat::default();
        let backend = audio_backend::find(Some("rodio".to_string())).unwrap();
        let mixer = Box::new(SoftMixer::open(MixerConfig::default()));
        // Iniciar com volume padrão de 80% (52428)
        mixer.set_volume(((0.8 * 65535.0) as u32).min(65535) as u16);
        let volume_getter = mixer.get_soft_volume();

        let (player, player_events) = Player::new(
            player_config,
            session.clone(),
            volume_getter,
            move || backend(None, audio_format),
        );

        Ok(Self { player, player_events, session, mixer })
    }

    /// Conecta com usuário e senha
    pub async fn new_with_password(username: String, password: String) -> Result<Self> {
        let credentials = Credentials::with_password(username, password);
        Self::new_with_credentials(credentials).await
    }

    /// Conecta usando o token de acesso OAuth PKCE
    pub async fn new_with_token(username: String, access_token: String) -> Result<Self> {
        let credentials = Credentials {
            username,
            auth_type: AuthenticationType::AUTHENTICATION_SPOTIFY_TOKEN,
            auth_data: access_token.into_bytes(),
        };
        Self::new_with_credentials(credentials).await
    }

    /// Loop principal do player que aguarda comandos e reporta progresso
    pub async fn run_command_loop(
        mut self,
        mut cmd_rx: mpsc::Receiver<PlayerCommand>,
        event_tx: mpsc::Sender<PlayerEventMsg>,
    ) {
        info!("Audio Player Command Loop iniciado.");
        let mut is_playing = false;
        let mut position_ms: u32 = 0;
        let mut last_tick = Instant::now();
        let mut ticker = tokio::time::interval(Duration::from_millis(250));

        loop {
            tokio::select! {
                cmd_opt = cmd_rx.recv() => {
                    match cmd_opt {
                        Some(PlayerCommand::Load { uri, play }) => {
                            info!("Carregando faixa: {}", uri);
                            if let Ok(id) = librespot_core::spotify_id::SpotifyId::from_uri(&uri) {
                                self.player.load(id, play, 0);
                                is_playing = play;
                                position_ms = 0;
                                last_tick = Instant::now();
                                let _ = event_tx.send(PlayerEventMsg::StateChanged { is_playing }).await;
                                let _ = event_tx.send(PlayerEventMsg::Position(0)).await;
                            } else {
                                error!("URI de faixa inválido: {}", uri);
                            }
                        }
                        Some(PlayerCommand::Play) => {
                            info!("Play");
                            self.player.play();
                            is_playing = true;
                            last_tick = Instant::now();
                            let _ = event_tx.send(PlayerEventMsg::StateChanged { is_playing: true }).await;
                        }
                        Some(PlayerCommand::Pause) => {
                            info!("Pause");
                            self.player.pause();
                            is_playing = false;
                            let _ = event_tx.send(PlayerEventMsg::StateChanged { is_playing: false }).await;
                        }
                        Some(PlayerCommand::Toggle) => {
                            if is_playing {
                                self.player.pause();
                                is_playing = false;
                            } else {
                                self.player.play();
                                is_playing = true;
                                last_tick = Instant::now();
                            }
                            let _ = event_tx.send(PlayerEventMsg::StateChanged { is_playing }).await;
                        }
                        Some(PlayerCommand::Seek(target_ms)) => {
                            info!("Seek para: {} ms", target_ms);
                            self.player.seek(target_ms);
                            position_ms = target_ms;
                            last_tick = Instant::now();
                            let _ = event_tx.send(PlayerEventMsg::Position(position_ms)).await;
                        }
                        Some(PlayerCommand::SetVolume(fraction)) => {
                            let clamped = fraction.clamp(0.0, 1.0);
                            let vol_u16 = ((clamped * 65535.0) as u32).min(65535) as u16;
                            info!("Ajustando volume para: {:.0}% ({})", clamped * 100.0, vol_u16);
                            self.mixer.set_volume(vol_u16);
                            self.player.emit_volume_set_event(vol_u16);
                        }
                        Some(PlayerCommand::Stop) => {
                            info!("Stop");
                            self.player.stop();
                            is_playing = false;
                            position_ms = 0;
                            let _ = event_tx.send(PlayerEventMsg::StateChanged { is_playing: false }).await;
                            let _ = event_tx.send(PlayerEventMsg::Position(0)).await;
                        }
                        None => {
                            info!("Canal de comandos do player encerrado.");
                            break;
                        }
                    }
                }
                event_opt = self.player_events.recv() => {
                    match event_opt {
                        Some(PlayerEvent::EndOfTrack { .. }) => {
                            info!("[AudioPlayer] PlayerEvent::EndOfTrack recebido do librespot! Avançando para próxima faixa.");
                            is_playing = false;
                            let _ = event_tx.send(PlayerEventMsg::StateChanged { is_playing: false }).await;
                            let _ = event_tx.send(PlayerEventMsg::EndOfTrack).await;
                        }
                        Some(PlayerEvent::Playing { position_ms: pos, .. }) => {
                            position_ms = pos;
                            last_tick = Instant::now();
                        }
                        Some(PlayerEvent::Paused { position_ms: pos, .. }) => {
                            position_ms = pos;
                            is_playing = false;
                        }
                        Some(PlayerEvent::Stopped { .. }) => {
                            is_playing = false;
                        }
                        None => {
                            info!("Canal de eventos do player librespot encerrado.");
                        }
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    if is_playing {
                        let elapsed = last_tick.elapsed().as_millis() as u32;
                        position_ms += elapsed;
                        last_tick = Instant::now();
                        let _ = event_tx.send(PlayerEventMsg::Position(position_ms)).await;
                    }
                }
            }
        }
    }
}
