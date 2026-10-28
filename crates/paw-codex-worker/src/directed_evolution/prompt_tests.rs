#[cfg(test)]
mod directed_evolution_prompt_tests {
    use super::*;

    #[test]
    fn evaluation_prompt_rewrites_loopback_api_base() {
        let body = directed_evolution_worker_prompt_body_with_public_api_url(
            "simulated_user",
            "Evaluate\nTemperApiBase: http://127.0.0.1:8080\nRuntimeRef: temper://tenant/t/app/a",
            Some("https://genesis.example.test/"),
        );

        assert!(body.contains("TemperApiBase: https://genesis.example.test"));
        assert!(body.contains("do not assume localhost is the target runtime"));
        assert!(body.contains("Do not start a foreground long-lived server"));
        assert!(!body.contains("TemperApiBase: http://127.0.0.1:8080"));
    }

    #[test]
    fn variant_generator_prompt_is_not_rewritten() {
        let body = directed_evolution_worker_prompt_body_with_public_api_url(
            "variant_generator",
            "TemperApiBase: http://127.0.0.1:8080",
            Some("https://genesis.example.test"),
        );

        assert_eq!(body, "TemperApiBase: http://127.0.0.1:8080");
    }
}
