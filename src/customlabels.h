/**
This file contains the TL portion of the TL context
sharing mechanism - that is, the bits that are actually
reflecting the state of the current thread.
**/

#ifndef CUSTOMLABELS_H
#define CUSTOMLABELS_H

#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>

//
// TL record: the per-thread context containing trace IDs and inline attributes.
//
// Users may hold multiple instances and swap them by updating the TL pointer.
// Users may also hold a _single_ instance, and simply set 'valid' to zero while
// the record is being updated.
//
typedef struct {
    // W3C trace context fields
    uint8_t trace_id[16];
    uint8_t span_id[8];

    // Readers should ignore this record if valid is 0
    _Atomic uint8_t valid;

    // Explicit padding for alignment of attrs_data_size
    uint8_t _padding;

    // Size of attrs_data in bytes (lets reader know when to stop parsing)
    uint16_t attrs_data_size;

    // Attribute data; each attr is [key_index:1][length:1][value:length]
    // This is stored without padding to a constant length (like the key table)
    // so that we squeeze as much data as we can into each context update.
    uint8_t attrs_data[];
} custom_labels_tl_record_t;

__attribute__((visibility("default")))
#ifdef __bindgen
extern __thread custom_labels_tl_record_t *otel_thread_ctx_v1;
#else
extern __thread _Atomic(custom_labels_tl_record_t *) otel_thread_ctx_v1;
#endif

// Initialize custom labels with the maximum record size.
// Must be called once before using any other functions.
void custom_labels_setup(uint64_t max_record_size);

// Get the configured max record size.
uint64_t custom_labels_get_max_record_size(void);

// Allocate a new TL record sized to max_record_size from setup()
custom_labels_tl_record_t *custom_labels_record_new(void);

// Free a TL record
void custom_labels_record_free(custom_labels_tl_record_t *record);

// Set trace context on a record
void custom_labels_record_set_trace(
    custom_labels_tl_record_t *record,
    const uint8_t trace_id[16],
    const uint8_t span_id[8]
);

// Add an attribute to a record. Returns 0 on success, -1 on error (e.g. buffer full)
int custom_labels_record_set_attr(
    custom_labels_tl_record_t *record,
    uint8_t key_index,
    const void *value,
    uint8_t value_length
);

// Set the current thread's active record; returns the previous record.
custom_labels_tl_record_t *custom_labels_set_current_record(
    custom_labels_tl_record_t *new_record
);

// Get the current thread's active record.
custom_labels_tl_record_t *custom_labels_get_current_record(void);

//get the address of the TLS variable itself (not its value)
void *custom_labels_get_tls_address(void);

#endif /* CUSTOMLABELS_H */
