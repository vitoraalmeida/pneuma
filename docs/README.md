# Documentação do Pneuma

Índice dos documentos e seus status. Regra: um documento **vivo** descreve o
sistema ou o trabalho atual e deve ser atualizado na mesma mudança que o
altera; um **registro** descreve algo executado e não muda; o **arquivo morto**
é histórico e não deve ser lido para entender o comportamento atual.

## Vivos

| Documento | Conteúdo |
|---|---|
| [`rust-guidelines.md`](rust-guidelines.md) | Convenções obrigatórias de código Rust |
| [`architecture/architecture.md`](architecture/architecture.md) | Arquitetura implementada: estrutura, runtime, exposição, máquina de estados |
| [`product/v0.1-scope.md`](product/v0.1-scope.md) | Contrato da v0.1: capacidades, aceite final, não objetivos |
| [`iterations/current-iteration.md`](iterations/current-iteration.md) | Iteração em andamento (o único acompanhamento de trabalho) |
| [`../roadmap.md`](../roadmap.md) | Direção de produto v0.1 → v0.3; leia apenas para planejar marcos |

## Registros operacionais

| Documento | Conteúdo |
|---|---|
| [`operations/staging-validation.md`](operations/staging-validation.md) | Validação manual do contrato do site em staging (ago/2026) |
| [`operations/public-deployment.md`](operations/public-deployment.md) | Procedimento e pré-requisitos do deployment público no host |

## Arquivo morto

[`archive/`](archive/) preserva as hipóteses da Iteração D0 (arquitetura
ports-and-adapters multi-crate, modelo de domínio, modelo de dados, plano ágil
e requisitos formais) que orientaram o início do projeto mas **não** descrevem
o sistema implementado. Consulte apenas para entender decisões históricas.
