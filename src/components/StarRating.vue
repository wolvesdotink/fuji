<script setup lang="ts">
import { ref } from "vue";

const props = withDefaults(
  defineProps<{
    rating: number;
    size?: "sm" | "md";
    readonly?: boolean;
  }>(),
  { size: "md", readonly: false }
);

const emit = defineEmits<{
  "update:rating": [rating: number];
}>();

const hoverRating = ref(0);

function onClick(star: number) {
  if (props.readonly) return;
  // Clicking the same star toggles it off (→ 0)
  emit("update:rating", star === props.rating ? 0 : star);
}
</script>

<template>
  <div
    :class="['star-rating', `star-${size}`]"
    @mouseleave="hoverRating = 0"
  >
    <button
      v-for="star in 5"
      :key="star"
      class="star-btn"
      :class="{
        filled: star <= (hoverRating || rating),
        hovering: hoverRating > 0 && star <= hoverRating,
      }"
      @click.stop="onClick(star)"
      @mouseenter="!readonly && (hoverRating = star)"
      :disabled="readonly"
      :tabindex="-1"
    >
      <svg viewBox="0 0 24 24" fill="currentColor">
        <path
          d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z"
        />
      </svg>
    </button>
  </div>
</template>

<style scoped>
.star-rating {
  display: inline-flex;
  align-items: center;
}

.star-md {
  gap: 3px;
}

.star-sm {
  gap: 2px;
}

.star-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  background: none;
  border: none;
  padding: 0;
  cursor: pointer;
  color: var(--color-border-hover);
  transition: color var(--transition-fast), transform var(--transition-fast);
}

.star-btn:disabled {
  cursor: default;
}

.star-md .star-btn svg {
  width: 18px;
  height: 18px;
}

.star-sm .star-btn svg {
  width: 14px;
  height: 14px;
}

.star-btn.filled {
  color: var(--color-accent);
}

.star-btn.hovering {
  color: var(--color-accent-hover);
}

.star-btn:not(:disabled):hover {
  transform: scale(1.15);
}

.star-btn:not(:disabled):active {
  transform: scale(0.95);
}
</style>
