#include <stdio.h>
#include <stdbool.h>
#include <unistd.h>
#include "firewheel-c.h"

int main() {
    printf("Creating Firewheel context...\n");
    FwContext* ctx = fw_context_create(NULL);
    if (ctx == NULL) {
        printf("Failed to create Firewheel context.\n");
        return 1;
    }
    printf("Firewheel context created successfully.\n");

    printf("\n--- Available Output Devices ---\n");
    FwAudioDeviceList* device_list = fw_audio_device_list_create(false);
    if (device_list != NULL) {
        uintptr_t len = fw_audio_device_list_get_len(device_list);
        for (uintptr_t i = 0; i < len; ++i) {
            const char* name = fw_audio_device_list_get_name(device_list, i);
            printf("%lu: %s\n", i, name);
        }
        fw_audio_device_list_free(device_list);
    }

    printf("\n--- Testing Node API ---\n");
    uint32_t beep_node_id = fw_node_add(ctx, FwFactoryNode_BeepTest);
    uint32_t volume_node_id = fw_node_add(ctx, FwFactoryNode_Volume);
    printf("Added BeepTest node with ID: %u\n", beep_node_id);
    printf("Added Volume node with ID: %u\n", volume_node_id);

    // Set volume
    uint32_t path_f32[] = {0};
    fw_node_set_f32_parameter(ctx, volume_node_id, path_f32, 1, 0.5f);

    // Connect beep node to volume
    uint32_t src_ports[] = {0};
    uint32_t dst_ports[] = {0};
    fw_node_connect(ctx, beep_node_id, volume_node_id, src_ports, dst_ports, 1);

    // Connect to volume node to output
    uint32_t graph_output_ports[] = {0, 1};
    fw_node_connect(ctx, volume_node_id, 0, src_ports, graph_output_ports, 1);

    printf("\n--- Playing beep for 1 seconds ---\n");
    uint32_t path_u32[] = {0};
    fw_node_set_u32_parameter(ctx, beep_node_id, path_u32, 1, 1);
    fw_context_update(ctx);
    sleep(1);
    fw_node_set_u32_parameter(ctx, beep_node_id, path_u32, 1, 0);
    fw_context_update(ctx);

    fw_context_free(ctx);
    printf("Firewheel context freed.\n");

    return 0;
}

