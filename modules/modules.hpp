#ifndef MODULES_HPP

#define MODULES_HPP

#include <forward_list>
#include <string>

class UI_module {
    public:
        std::string label;
};

typedef std::forward_list<UI_module > ui_module_list_t;

class NeomasterModule {
    public:
        UI_module *ui_module = nullptr;
};

typedef std::forward_list<NeomasterModule> module_list_t;

class ModuleManager {
    public:
        ModuleManager();
        module_list_t module_list;
        ui_module_list_t get_ui_modules();
};




#endif
