O Vulcan Code 0.1.25 melhora a inicialização no desktop, as bases de conhecimento e os assistentes dedicados. O instalador independente 0.2.13 permite escolher a rota de download.

### Inicialização e ambiente de execução

```ui-checklist
ok | Progresso da inicialização | A tela inicial mostra a etapa atual do Core e o nome da habilidade em download.
ok | Compatibilidade com LuaSkills | O Core usa o ambiente e os caminhos de módulos Lua incluídos, sem interferência das variáveis de ambiente herdadas. Também corrige o carregamento de módulos em caminhos de instalação do Windows com caracteres que não sejam ASCII.
```

### Bases de conhecimento

```ui-checklist
ok | Reutilização de índices | Alterar o modo de busca preserva os índices e as configurações de embeddings. Ao reativar o mesmo espaço vetorial, só o conteúdo ausente é processado.
ok | Prévia e resultados de busca | A aplicação web mostra documentos atuais e históricos em Markdown, com opção de ver o original. Os resumos abrem o início do documento e as correspondências no texto abrem a linha correspondente.
```

### Assistentes e modelos

```ui-checklist
ok | Memória bloqueada | A conversão abre o assistente flutuante e mostra o estado do bloqueio. Rodadas concluídas com a memória bloqueada indicam que o conteúdo não será levado à próxima rodada.
ok | Instruções do sistema | As alterações nas instruções base são aplicadas na próxima execução, antes do histórico, sem precisar recarregar a sessão.
```

### Instalador independente 0.2.13

```ui-checklist
ok | Rotas de download | Escolha Automática, Internacional ou China continental. O instalador salva a escolha, que também pode ser definida com `--route`.
```
