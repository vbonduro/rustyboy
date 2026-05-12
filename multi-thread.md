I would like to offload the apu and pppu onto core1. The two tasks on the cores will have to share GameboyMemory by using a mutex around access points. 

Risks with this include how to handle borrow checking. I was thinking we can isolate the regions of shared memory into a mitex struct and make it easy for both cores to access it.

The multithread design in the core library must work with webapp and pico2w. 

the ppu and apu in core 1 must operate independentlt as possible from core 0. core 0 tasks will not wait on core 1, other than waiting on mutex lock, but only where needed.

core0 will send a signal to core 1 to tell it how many ticks/cycles to run for ppu/apu. core1 will wait on that signal then call the respective advance functions.

lets start by listing the shared memory and who is a writer/reader of each. then plan out the refactor.
