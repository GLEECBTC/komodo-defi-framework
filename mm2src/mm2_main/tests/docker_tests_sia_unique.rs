#![feature(custom_test_frameworks)]
#![feature(test)]
#![cfg(feature = "enable-sia")]
#![cfg(not(target_arch = "wasm32"))]
#![allow(unused_imports, dead_code)]
#![test_runner(docker_tests_runner)]

#[cfg(test)]
#[macro_use]
extern crate common;
#[cfg(test)]
#[macro_use]
extern crate gstuff;
#[cfg(test)]
#[macro_use]
extern crate lazy_static;
#[cfg(test)]
#[macro_use]
extern crate serde_json;
#[cfg(test)]
extern crate ser_error_derive;
#[cfg(test)]
extern crate test;

use std::env;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use test::{test_main, StaticBenchFn, StaticTestFn, TestDescAndFn};
use testcontainers::clients::Cli;

mod docker_tests;
use docker_tests::docker_tests_common::*;

#[allow(dead_code)]
mod integration_tests_common;

/// Custom test runner intended to initialize the SIA coin daemon in a Docker container.
pub fn docker_tests_runner(tests: &[&TestDescAndFn]) {
    let docker = Cli::default();
    let mut containers = vec![];

    let skip_docker_tests_runner = std::env::var("SKIP_DOCKER_TESTS_RUNNER")
        .map(|v| v == "1")
        .unwrap_or(false);

    if !skip_docker_tests_runner {
        const IMAGES: &[&str] = &[SIA_DOCKER_IMAGE_WITH_TAG];

        for image in IMAGES {
            pull_docker_image(image);
            remove_docker_containers(image);
        }

        let sia_node = sia_docker_node(&docker, "SIA", 9980);
        println!("ran container?");
        containers.push(sia_node);
    }
    // detect if docker is installed
    // skip the tests that use docker if not installed
    let owned_tests: Vec<_> = tests
        .iter()
        .map(|t| match t.testfn {
            StaticTestFn(f) => TestDescAndFn {
                testfn: StaticTestFn(f),
                desc: t.desc.clone(),
            },
            StaticBenchFn(f) => TestDescAndFn {
                testfn: StaticBenchFn(f),
                desc: t.desc.clone(),
            },
            _ => panic!("non-static tests passed to lp_coins test runner"),
        })
        .collect();
    let args: Vec<String> = env::args().collect();
    test_main(&args, owned_tests, None);
}

fn pull_docker_image(name: &str) {
    Command::new("docker")
        .arg("pull")
        .arg(name)
        .status()
        .expect("Failed to execute docker command");
}

fn remove_docker_containers(name: &str) {
    let stdout = Command::new("docker")
        .arg("ps")
        .arg("-f")
        .arg(format!("ancestor={name}"))
        .arg("-q")
        .output()
        .expect("Failed to execute docker command");

    let reader = BufReader::new(stdout.stdout.as_slice());
    let ids: Vec<_> = reader.lines().map(|line| line.unwrap()).collect();
    if !ids.is_empty() {
        Command::new("docker")
            .arg("rm")
            .arg("-f")
            .args(ids)
            .status()
            .expect("Failed to execute docker command");
    }
}
