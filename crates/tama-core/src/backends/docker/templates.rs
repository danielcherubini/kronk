/// Built-in Docker compose templates for common inference backends.
///
/// Templates include vLLM (ROCm/AITER), vLLM (CUDA), llama.cpp, and a
/// custom blank template.

/// A built-in Docker template.
pub struct Template {
    /// Display name shown in the UI.
    pub name: &'static str,
    /// Description shown in the UI.
    pub description: &'static str,
    /// Default port for the inference server.
    pub default_port: u16,
    /// The compose YAML template string.
    pub compose_yaml: &'static str,
}

/// Return the list of built-in templates.
pub fn available_templates() -> &'static [Template] {
    &[
        Template {
            name: "vLLM (ROCm/AITER)",
            description: "vLLM with AMD ROCm/AITER optimized attention backends for AMD GPUs.",
            default_port: 8000,
            compose_yaml: VLLM_ROCM_TEMPLATE,
        },
        Template {
            name: "vLLM (CUDA)",
            description: "vLLM with NVIDIA CUDA support for NVIDIA GPUs.",
            default_port: 8000,
            compose_yaml: VLLM_CUDA_TEMPLATE,
        },
        Template {
            name: "llama.cpp",
            description: "Official llama.cpp Docker image for CPU and GPU inference.",
            default_port: 8080,
            compose_yaml: LLAMA_CPP_TEMPLATE,
        },
        Template {
            name: "Custom",
            description: "Blank template — write your own compose YAML.",
            default_port: 8000,
            compose_yaml: "",
        },
    ]
}

/// vLLM ROCm/AITER template with placeholders.
const VLLM_ROCM_TEMPLATE: &str = r#"services:
  vllm:
    image: aml731/vllm-aiter:v0.19.1
    network_mode: host
    group_add:
      - video
    ipc: host
    cap_add:
      - SYS_PTRACE
    security_opt:
      - seccomp:unconfined
    devices:
      - /dev/kfd:/dev/kfd
      - /dev/dri:/dev/dri
    volumes:
      - {volume_path}:/data/models
    environment:
      - VLLM_ROCM_USE_AITER=1
      - VLLM_ROCM_ALLOW_RDNA4_AITER_ATTENTION=1
      - VLLM_ROCM_USE_AITER_UNIFIED_ATTENTION=1
      - VLLM_ROCM_USE_AITER_MHA=0
      - VLLM_ROCM_USE_AITER_PAGED_ATTN=0
      - VLLM_ROCM_USE_AITER_MOE=0
      - VLLM_ROCM_USE_AITER_LINEAR=0
      - FLASH_ATTENTION_TRITON_AMD_ENABLE=TRUE
      - PYTORCH_ALLOC_CONF=expandable_segments:True
    command: >
      python3 -m vllm.entrypoints.openai.api_server
      --model {model_path}
      --tensor-parallel-size {tp_size}
      --dtype auto
      --attention-backend ROCM_AITER_UNIFIED_ATTN
      --max-model-len 131072
      --gpu-memory-utilization 0.95
      --enable-prefix-caching
      --trust-remote-code
      --quantization fp8
      --host 0.0.0.0
      --port 8000
"#;

/// vLLM CUDA template.
const VLLM_CUDA_TEMPLATE: &str = r#"services:
  vllm:
    image: vllm/vllm-openai:latest
    runtime: nvidia
    network_mode: host
    volumes:
      - {volume_path}:/data/models
    environment:
      - NVIDIA_VISIBLE_DEVICES=all
    command: >
      python3 -m vllm.entrypoints.openai.api_server
      --model {model_path}
      --tensor-parallel-size {tp_size}
      --dtype auto
      --max-model-len 131072
      --gpu-memory-utilization 0.95
      --enable-prefix-caching
      --trust-remote-code
      --host 0.0.0.0
      --port 8000
"#;

/// llama.cpp template.
const LLAMA_CPP_TEMPLATE: &str = r#"services:
  llama-cpp:
    image: ghcr.io/ggml-org/llama.cpp:latest
    network_mode: host
    volumes:
      - {volume_path}:/data/models
    command: >
      ./server
      -m /data/models/{model_path}
      --host 0.0.0.0
      --port 8080
      --n-gpu-layers 999
      --ctx-size 131072
"#;
