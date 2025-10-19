/*
 * Copyright The OpenTelemetry Authors
 * SPDX-License-Identifier: Apache-2.0
 */

//
// bpf_memory.h - BPF memory related utility functions
//

#pragma once

#include "bpf_debug.h"
#include "bpf_types.h"

// s2 can not be longer than 16 bytes due to older bpf inlining limitations
static __always_inline int string_starts_with(const char *s1, const size_t s1_len, const char *s2)
{
  // Load needle (s2) from kernel memory into a bounded buffer, using
  // bpf_probe_read_kernel_str to obtain its length. The returned length
  // includes the trailing NUL when successful.
  char s2_local[16] = {};
  int n2 = (int)bpf_probe_read_kernel_str(s2_local, sizeof(s2_local), s2);
  if (n2 <= 0) {
    return 0;
  }

  // Drop the trailing NUL; compute the effective needle length.
  size_t needle_len = (size_t)(n2 - 1);
  if (needle_len == 0) {
    // Empty needle trivially matches, but we don't expect empty needles here.
    return 1;
  }
  if (needle_len > sizeof(s2_local)) {
    // Help the verifier; should never happen due to buffer size.
    return 0;
  }

  // Ensure the haystack has enough bytes available.
  if (s1_len < needle_len) {
    return 0;
  }

  // Read exactly needle_len bytes from user memory at s1 into a bounded buffer.
  char s1_local[16] = {};
  if (bpf_probe_read(s1_local, needle_len, s1) != 0) {
    return 0;
  }

  // Bounded compare up to 16 bytes to keep the verifier happy.
  for (int i = 0; i < 16; i++) {
    if ((size_t)i == needle_len) {
      return 1;
    }
    if (s2_local[i] != s1_local[i]) {
      return 0;
    }
  }

  return 1;
}

static __always_inline int char_to_number(char x)
{
  if (x < '0' || x > '9')
    return -1;
  return (int)(x - '0');
}
