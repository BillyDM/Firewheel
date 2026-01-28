#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * An enumeration of the built-in factory nodes.
 */
enum FwFactoryNode {
  FwFactoryNode_BeepTest,
  FwFactoryNode_Sampler,
  FwFactoryNode_SVF,
  FwFactoryNode_Volume,
  FwFactoryNode_VolumePan,
};
typedef uint32_t FwFactoryNode;

/**
 * An enumeration of the fields that can be set in a stream configuration.
 */
enum FwStreamConfigField {
  FwStreamConfigField_Input,
  FwStreamConfigField_Output,
  FwStreamConfigField_SampleRate,
};
typedef uint32_t FwStreamConfigField;

/**
 * Opaque struct for FirewheelConfig.
 */
typedef struct FwConfig {
  uint8_t _private[0];
} FwConfig;

/**
 * Opaque struct for the audio context.
 */
typedef struct FwContext {
  uint8_t _private[0];
} FwContext;

/**
 * Opaque struct for a loaded sample
 */
typedef struct FwSample {
  uint8_t _private[0];
} FwSample;

/**
 * Opaque struct for stream configuration.
 */
typedef struct FwStreamConfig {
  uint8_t _private[0];
} FwStreamConfig;

/**
 * A union of the possible values for a stream configuration field.
 */
typedef union FwStreamConfigValue {
  const char *device_name;
  uint32_t sample_rate;
  uint32_t num_channels;
} FwStreamConfigValue;

/**
 * Opaque struct for a list of audio devices.
 */
typedef struct FwAudioDeviceList {
  uint8_t _private[0];
} FwAudioDeviceList;

const char *fw_get_last_error(void);

/**
 * Create a new Firewheel config with default values.
 */
struct FwConfig *fw_config_create(void);

/**
 * Free a Firewheel config.
 */
void fw_config_free(struct FwConfig *config);

/**
 * Set the number of graph inputs in a Firewheel config.
 */
void fw_config_set_num_graph_inputs(struct FwConfig *config, uint32_t value);

/**
 * Set the number of graph outputs in a Firewheel config.
 */
void fw_config_set_num_graph_outputs(struct FwConfig *config, uint32_t value);

/**
 * Set whether outputs should be hard clipped in a Firewheel config.
 */
void fw_config_set_hard_clip_outputs(struct FwConfig *config, int value);

/**
 * Set the declick seconds in a Firewheel config.
 */
void fw_config_set_declick_seconds(struct FwConfig *config, float value);

/**
 * Create a new audio context with the given config.
 * If `config` is NULL, a default config will be used.
 */
struct FwContext *fw_context_create(struct FwConfig *config);

/**
 * Free an audio context.
 */
void fw_context_free(struct FwContext *ctx);

/**
 * Set the playing state of the context's musical transport.
 */
void fw_context_set_playing(struct FwContext *ctx, bool playing);

/**
 * Get the playing state of the context's musical transport.
 * Returns true if playing, false otherwise.
 */
bool fw_context_is_playing(struct FwContext *ctx);

/**
 * Set the static beats per minute (BPM) of the context's musical transport.
 * If bpm is 0.0 or negative, the static transport will be unset.
 */
void fw_context_set_static_beats_per_minute(struct FwContext *ctx, double bpm);

/**
 * Get the static beats per minute (BPM) of the context's musical transport.
 * Returns 0.0 if no static transport is set or if musical_transport feature is not enabled.
 */
double fw_context_get_beats_per_minute(struct FwContext *ctx);

/**
 * Set the musical playhead of the context's musical transport.
 */
void fw_context_set_playhead(struct FwContext *ctx, double playhead_musical);

/**
 * Get the musical playhead of the context's musical transport.
 * Returns 0.0 if musical_transport feature is not enabled.
 */
double fw_context_get_playhead(struct FwContext *ctx);

/**
 * Set the speed multiplier of the context's musical transport.
 */
void fw_context_set_speed_multiplier(struct FwContext *ctx, double multiplier);

/**
 * Remove a node from the Firewheel context.
 * Returns 0 on success, -1 on error.
 */
int fw_node_remove(struct FwContext *ctx, uint32_t node_id);

/**
 * Add a new factory node to the Firewheel context.
 * Returns the ID of the new node on success, or 0 on error.
 */
uint32_t fw_node_add(struct FwContext *ctx, FwFactoryNode node_type);

/**
 * Connects two nodes in the Firewheel context.
 *
 * `src_ports` and `dst_ports` are arrays of port indices.
 * The length of both arrays must be `num_ports`.
 *
 * Returns 0 on success, -1 on error.
 */
int fw_node_connect(struct FwContext *ctx,
                    uint32_t src_node,
                    uint32_t dst_node,
                    const uint32_t *src_ports,
                    const uint32_t *dst_ports,
                    uintptr_t num_ports);

/**
 * Set an f32 parameter on a node.
 * Returns 0 on success, -1 on error.
 */
int fw_node_set_f32_parameter(struct FwContext *ctx,
                              uint32_t node_id,
                              const uint32_t *path_indices,
                              uintptr_t path_len,
                              float value);

/**
 * Set a u32 parameter on a node.
 * Returns 0 on success, -1 on error.
 */
int fw_node_set_u32_parameter(struct FwContext *ctx,
                              uint32_t node_id,
                              const uint32_t *path_indices,
                              uintptr_t path_len,
                              uint32_t value);

/**
 * Update the Firewheel context, processing any pending events.
 */
void fw_context_update(struct FwContext *ctx);

/**
 * Disconnects two nodes in the Firewheel context.
 *
 * `src_ports` and `dst_ports` are arrays of port indices.
 * The length of both arrays must be `num_ports`.
 */
int fw_node_disconnect(struct FwContext *ctx,
                       uint32_t src_node,
                       uint32_t dst_node,
                       const uint32_t *src_ports,
                       const uint32_t *dst_ports,
                       uintptr_t num_ports);

/**
 * Load a sample from a file path.
 * Returns a pointer to an FwSample on success, or NULL on error.
 * Use fw_get_last_error to retrieve the error message.
 */
struct FwSample *fw_sample_load_from_file(const char *path);

/**
 * Free a loaded sample.
 */
void fw_sample_free(struct FwSample *sample);

/**
 * Create a new stream configuration with default values.
 */
struct FwStreamConfig *fw_stream_config_create(void);

/**
 * Free a stream configuration.
 */
void fw_stream_config_free(struct FwStreamConfig *config);

/**
 * Set a field in a stream configuration.
 */
void fw_stream_config_set(struct FwStreamConfig *config,
                          FwStreamConfigField field,
                          union FwStreamConfigValue value);

/**
 * Create a list of available audio devices.
 */
struct FwAudioDeviceList *fw_audio_device_list_create(bool input);

/**
 * Free a list of audio devices.
 */
void fw_audio_device_list_free(struct FwAudioDeviceList *list);

/**
 * Get the number of audio devices in a list.
 */
uintptr_t fw_audio_device_list_get_len(struct FwAudioDeviceList *list);

/**
 * Get the name of an audio device from a list
 */
const char *fw_audio_device_list_get_name(struct FwAudioDeviceList *list, uintptr_t index);
