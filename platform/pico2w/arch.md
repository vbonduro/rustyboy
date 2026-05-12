# Overview

The current architecture has a tightly coupled cpu 
controlling all the peripherals and timing their
ticks. This creates a tightly coupled system that
is difficult to transition to a more threaded or
pipelined model.

instead we want to create a new GameBoy class. The
GameBoy will own all of the components (cpu, mem,
timers, ppu, apu, etc.) and will be responsible for
ticking all of them. 

In addition, we need to audit the ppu and apu to
ensure that they own their own registers and 
memory regions like in a real gameboy. Special care
will need to be taken for any memory shared between
peripherals and the cpu. This memory could be owned
by the Gameboy and it can pass it as a borrowed 
reference to the respective components tick 
function. Also open to other ideas for this

After this rework, the cpu should no longer own
all the pieces of the gameboy and should be repsonsible for instruction decode and dispatch.
Each peripheral should be independent of eachother as much as possible
