# Planejamento Arquitetural: Spotify Rust Nativo (Ultra-leve)

## Visão Geral da Arquitetura

O sistema será dividido em três camadas principais operando em threads separadas para garantir ausência de engasgos e interface fluida.

```mermaid
flowchart TD
    subgraph UI["UI Thread (Slint)"]
        UI_Core[Componentes Visuais]
        UI_State[Gerenciador de Estado]
    end

    subgraph Core["Network & Worker Pool (Tokio)"]
        API[Reqwest / RSpotify]
        Cache[(SQLite / rusqlite)]
        Auth[OAuth / PKCE]
    end

    subgraph Audio["Audio / Stream Thread"]
        Librespot[librespot-core / audio]
        Buffer[Buffer Circular]
        Rodio[rodio (Audio Sink)]
    end

    UI_State <-->|Canais mpsc / Metadados| Core
    UI_State <-->|Comandos (Play/Pause)| Audio
    Core <-->|Busca de Faixas/Capas| API
    Core <-->|Leitura/Escrita Local| Cache
    Audio <-->|Streaming Ogg Vorbis| Librespot
    Audio -->|Saída PCM| Rodio
```

## Estrutura de Pastas e Módulos do Rust

```text
spotify-rust/
├── Cargo.toml
├── build.rs             # Para compilar views do Slint (caso escolhido)
├── src/
│   ├── main.rs          # Ponto de entrada principal
│   ├── ui/              # Componentes da interface, layout e estado visual
│   ├── audio/           # Integração com librespot, cpal/rodio, gerenciamento de buffer
│   ├── api/             # Comunicação com Spotify Web API, autenticação e tokens
│   ├── cache/           # Banco de dados SQLite, armazenamento de imagens e histórico
│   ├── models/          # Estruturas de dados compartilhadas (Track, Album, Playlist)
│   └── utils/           # Utilitários (formatação de tempo, logs, conversões)
```

## Checklist Executável por Fases

### Fase 1: Fundação e Autenticação
- [x] Inicializar o projeto `cargo init` e configurar `Cargo.toml`.
- [x] Implementar a estrutura de diretórios e módulos.
- [x] Configurar autenticação OAuth PKCE (Spotify API) e sessão (librespot).
- [x] Persistir tokens e chaves de sessão de forma segura.

### Fase 2: Engine de Áudio e Streaming
- [x] Integrar `librespot` e resolver login da sessão do player.
- [x] Configurar recebimento de stream Ogg Vorbis num buffer circular lock-free (resolvido nativamente via sink `rodio` no librespot).
- [x] Inicializar engine de saída (`rodio`) com alta prioridade de tempo real.
- [x] Conectar os buffers e implementar comandos de Play, Pause e Seek (via `PlayerCommand`).

### Fase 3: Interface Gráfica (GUI) Nativa
- [x] Configurar o shell inicial da janela utilizando **Slint GUI**.
- [x] Construir componentes base: Player no Topo, Barra Lateral e Conteúdo (Estilo Apple Music).
- [x] Implementar funções nativas: Layout de Letras em tempo real e botão de Compartilhar.
- [x] Implementar a ponte de canais `tokio::sync::mpsc` entre a UI e os Workers (rede/áudio).

### Fase 4: Dados e Cache Local
- [x] Configurar banco de dados local SQLite com `rusqlite` e schemas iniciais.
- [x] Integrar API de Letras Abertas (`LRCLIB.net`) em formato LRC para suportar o recurso de visualização em tempo real (sem restrições da API oficial).
- [x] Implementar cache em disco de capas de álbuns, evitando sobrecarga na rede e memória.
- [x] Preparar estrutura para busca e paginação de dados do Spotify (Playlists, Álbuns e Faixas).

### Fase 5: Otimização e Empacotamento Final
- [ ] Profiling rigoroso de uso de RAM e CPU (objetivo: < 40MB).
- [ ] Otimizações no profile release (`lto`, `strip`, `codegen-units`, otimização de tamanho).
- [ ] Tratamento de reconexões, modo offline e exceções de áudio no SO.
- [ ] Empacotamento do binário final nativo para Windows.

## Análise de Gargalos e Soluções

1. **Gargalo: Engasgos no Áudio (Stuttering)**
   - *Causa:* Thread de interface ou rede competindo por recursos, bloqueando a entrega de amostras ao dispositivo de áudio (WASAPI).
   - *Solução:* Separar fisicamente a thread de áudio e atribuir prioridade crítica do SO. Implementar uma fila lock-free (como `rtrb` ou `crossbeam-queue`) preenchida por blocos decodificados na thread de rede e puramente consumida pela thread de saída.

2. **Gargalo: Consumo Exacerbado de RAM pela UI**
   - *Causa:* Instanciar e renderizar todos os nós visuais e capas de uma playlist com mais de 10.000 músicas simultaneamente.
   - *Solução:* Renderização estrita virtualizada. O framework GUI apenas atualizará (reciclará) as dezenas de instâncias que de fato aparecem na tela, mapeando para as estruturas na memória e buscando imagens via cache sob demanda.

3. **Gargalo: Tamanho e Overhead de Inicialização**
   - *Causa:* Bibliotecas estáticas muito grandes, inicialização síncrona bloqueante.
   - *Solução:* Excluir qualquer engrenagem Web (descarte total de CEF/Electron/Tauri). Executar boot da aplicação focando apenas na renderização da tela em poucos milissegundos e iniciar os módulos de rede/áudio e autenticação de forma atrasada (*lazy init*) via tarefas do Tokio.
