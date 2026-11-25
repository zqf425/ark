use ark_common::{ItemId, ThreadId, TurnId};
use ark_contract::RuntimeEvent;
use ark_store::project_items;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn rebuilding_delta_text_does_not_clone_the_accumulated_item_per_delta() {
    let thread = ThreadId::new();
    let turn = TurnId::new();
    let item = ItemId::new();
    let chunk = "x".repeat(1024);
    let events: Vec<RuntimeEvent> = (1..=384)
        .map(|seq| RuntimeEvent::AssistantTextDelta {
            seq,
            thread_id: thread,
            turn_id: turn,
            item_id: item,
            timestamp: chrono::Utc::now(),
            delta: chunk.clone(),
        })
        .collect();

    ALLOCATED.store(0, Ordering::Relaxed);
    let projected = project_items(&events);
    let allocated = ALLOCATED.load(Ordering::Relaxed);

    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].text.as_deref().unwrap().len(), 384 * 1024);
    assert!(
        allocated < 8 * 1024 * 1024,
        "delta projection allocated {allocated} bytes; full-item clones make this quadratic"
    );
}
