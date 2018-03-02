CC=cc
CXX=g++
RM=rm -f
CPPFLAGS=-g -fPIC -std=gnu++11 -Wall -Wextra -I. $(shell pkg-config --cflags libusb-1.0 sdl2 SDL2_ttf SDL2_image)
LDFLAGS=-g 
LDLIBS=/usr/lib/x86_64-linux-gnu/libboost_log.a $(shell pkg-config --libs libusb-1.0 sdl2 SDL2_ttf SDL2_image) -lboost_iostreams -lboost_thread -lboost_system -lpthread -lboost_log

PROG=neomaster
SRCS=$(wildcard modules/*.cpp) $(wildcard ui/*.cpp)  neomaster.cpp
OBJS=$(subst .cpp,.o,$(SRCS)) 

all:		$(PROG)

neomaster:	$(OBJS)
		$(CXX) $(LDFLAGS) -o neomaster $(OBJS) $(LDLIBS) 

clean:
	    $(RM) $(OBJS)

distclean: clean
	    $(RM) neomaster
