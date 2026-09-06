# Plano de Implementação: Spotify Rust com Estilo Apple Music

Este plano detalha as etapas para transformar o protótipo atual em um cliente Spotify funcional com a estética do Apple Music.

## 2. Autenticação e Integração Spotify (Prioridade Alta)
- **OAuth PKCE via Navegador:**
    - Implementar o fluxo de autorização usando `rspotify`.
    - Configurar um servidor HTTP local temporário (usando `tiny_http` ou similar) em `http://127.0.0.1:8888/callback` para capturar o código de autorização automaticamente.
    - O app abrirá o navegador padrão, o usuário fará login, e o navegador redirecionará de volta para o app, fechando a aba em seguida.
    - Persistir o `refresh_token` de forma segura para evitar logins repetidos.
- **Sessão Unificada:**
    - Usar as credenciais obtidas para inicializar o `librespot` (Spotify Connect) e permitir o streaming de áudio.

## 1. Refatoração da Interface (Estilo Apple Music)
- **Identidade Visual:**
    - Ajustar cores para o padrão Apple (Blurs, gradientes sutis no player).
    - Implementar a visualização de "Letras em Tela Cheia" com o fundo dinâmico baseado na cor da capa do álbum (estilo Apple Music).
- **Componentização no Slint:**
    - Separar a interface em arquivos menores para facilitar a manutenção.
    - Implementar a barra de busca funcional na sidebar.
    - Adicionar animações suaves de transição entre a visualização de biblioteca e letras.
- **Estado da UI:**
    - Definir structs no Rust que mapeiem para modelos no Slint (`TrackModel`, `PlaylistModel`).
    - Usar `VecModel` do Slint para exibir listas dinâmicas de músicas.

## 3. Lógica de Negócio e Streaming
- **Gerenciamento de Playlist:**
    - Implementar busca de playlists do usuário ao iniciar.
    - Suporte para carregar e tocar uma música ao clicar.
- **Controle de Playback:**
    - Sincronizar o slider de progresso da UI com a posição real do `librespot`.
    - Implementar controle de volume real.
- **Cache de Capas:**
    - Implementar o `src/cache/images.rs` para baixar e persistir capas de álbuns localmente, reduzindo consumo de banda.

## 4. Letras em Tempo Real (Apple Music Style)
- **Sincronização LRC:**
    - Parsear o formato LRC retornado pelo `LRCLIB`.
    - Criar um timer no Rust que envia a linha atual da letra para a UI baseado no timestamp do áudio.
    - Implementar o efeito de "foco" na linha ativa (maior e mais clara) e "desfoque" nas outras, típico do Apple Music.

## 5. Otimização e Finalização
- **Profiling de Memória:** Garantir que o app se mantenha leve (< 40MB RAM).
- **Tratamento de Erros:** Adicionar notificações na UI para falhas de conexão ou autenticação.
- **Binário Nativo:** Configurar o workflow de build para gerar um executável otimizado para Windows.

## Verificação
- [ ] O app abre e solicita login caso não haja token.
- [ ] É possível navegar pelas playlists do usuário.
- [ ] O player toca música com controles de play/pause/skip.
- [ ] As letras aparecem sincronizadas com a música.
- [ ] A interface se assemelha visualmente ao Apple Music (Cores, Fontes, Layout).
