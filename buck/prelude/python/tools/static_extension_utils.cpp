/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

#include <string>
#include <string_view>
#include <unordered_map>
#include "Python.h"

typedef PyObject* (*pyinitfunc)();

typedef struct {
} StaticExtensionFinderObject;

extern std::unordered_map<std::string_view, pyinitfunc> _static_extension_info;

namespace {

static PyObject* _create_module(PyObject* self, PyObject* spec) {
  PyObject* name;
  PyObject* mod;
  const char* oldcontext;

  name = PyObject_GetAttrString(spec, "name");
  if (name == nullptr) {
    return nullptr;
  }

  // TODO private api usage
  mod = _PyImport_FindExtensionObject(name, name);
  if (mod || PyErr_Occurred()) {
    Py_DECREF(name);
    Py_XINCREF(mod);
    return mod;
  }

  const std::string namestr = PyUnicode_AsUTF8(name);
  if (namestr.empty()) {
    Py_DECREF(name);
    return nullptr;
  }

  pyinitfunc initfunc = nullptr;
  if (auto result = _static_extension_info.find(namestr);
      result != _static_extension_info.end())
    initfunc = result->second;

  if (initfunc == nullptr) {
    PyErr_SetString(
        PyExc_ImportError, "Module unknown to static extension finder");
    return nullptr;
  }

  PyObject* modules = nullptr;
  PyModuleDef* def;
  oldcontext = _Py_PackageContext;
  _Py_PackageContext = namestr.c_str();
  if (_Py_PackageContext == nullptr) {
    _Py_PackageContext = oldcontext;
    Py_DECREF(name);
    return nullptr;
  }
  mod = initfunc();
  _Py_PackageContext = oldcontext;
  if (mod == nullptr) {
    Py_DECREF(name);
    return nullptr;
  }
  if (PyObject_TypeCheck(mod, &PyModuleDef_Type)) {
    Py_DECREF(name);
    return PyModule_FromDefAndSpec((PyModuleDef*)mod, spec);
  } else {
    /* Remember pointer to module init function. */
    def = PyModule_GetDef(mod);
    if (def == nullptr) {
      Py_DECREF(name);
      return nullptr;
    }
    PyObject* path = PyObject_GetAttrString(spec, "origin");
    if (PyModule_AddObject(mod, "__file__", path) < 0) {
      PyErr_Clear();
    } else {
      Py_INCREF(path);
    }
    def->m_base.m_init = initfunc;
    if (modules == nullptr) {
      modules = PyImport_GetModuleDict();
    }
    // TODO private api usage
    if (_PyImport_FixupExtensionObject(mod, name, name, modules) < 0) {
      Py_DECREF(path);
      Py_DECREF(name);
      return nullptr;
    }
    Py_DECREF(path);
    Py_DECREF(name);
    return mod;
  }
  Py_DECREF(name);
  Py_RETURN_NONE;
}

static PyObject* _exec_module(PyObject* self, PyObject* module) {
  PyModuleDef* def;
  int res;

  // TODO errors
  if (!PyModule_Check(module)) {
    // TODO
    Py_RETURN_NONE;
  }

  def = PyModule_GetDef(module);
  if (def == nullptr) {
    // TODO
    Py_RETURN_NONE;
  }

  res = PyModule_ExecDef(module, def);
  // TODO check res
  Py_RETURN_NONE;
}

PyDoc_STRVAR(
    StaticExtensionLoader_doc,
    "static_extension_loader(name: str)\n\
\n\
a loader for extensions linked statically into the binary");

static PyMethodDef StaticExtensionLoaderType_methods[] = {
    {"create_module",
     (PyCFunction)(void (*)(void))_create_module,
     METH_STATIC | METH_O,
     nullptr},
    {"exec_module",
     (PyCFunction)(void (*)(void))_exec_module,
     METH_STATIC | METH_O,
     nullptr},
    {nullptr, nullptr}};

static PyType_Slot StaticExtensionLoader_slots[] = {
    {Py_tp_doc, (void*)StaticExtensionLoader_doc},
    {Py_tp_methods, StaticExtensionLoaderType_methods},
    {0, 0}};

static PyType_Spec StaticExtensionLoader_spec = {
    "static_extension_utils.StaticExtensionLoader",
    0,
    0,
    Py_TPFLAGS_DEFAULT,
    StaticExtensionLoader_slots};

static int _static_extension_utils_exec(PyObject* m) {
  PyObject* loader_type = PyType_FromSpec(&StaticExtensionLoader_spec);
  if (loader_type == nullptr) {
    return -1;
  }
  int result = PyModule_AddObject(m, "StaticExtensionLoader", loader_type);
  if (result == -1) {
    Py_DECREF(loader_type);
    return -1;
  }
  return 0;
}

PyDoc_STRVAR(
    _check_module_doc,
    "Check if a module is contained in the C Extension map \n");

static PyObject* _check_module(PyObject* self, PyObject* fullname) {
  if (!PyUnicode_Check(fullname)) {
    PyErr_SetString(PyExc_TypeError, "Expected a unicode object");
    return nullptr;
  }
  const std::string modname = PyUnicode_AsUTF8(fullname);
  if (_static_extension_info.find(modname) != _static_extension_info.end()) {
    Py_INCREF(Py_True);
    return Py_True;
  }
  Py_INCREF(Py_False);
  return Py_False;
}

static PyModuleDef_Slot _static_extension_utils_slots[] = {
    {Py_mod_exec, (void*)_static_extension_utils_exec},
    {0, nullptr}};

static PyMethodDef _static_extension_utils_methods[] = {
    {"_check_module", _check_module, METH_O, _check_module_doc},
    {nullptr, nullptr}};

PyDoc_STRVAR(
    module_doc,
    "Utils for importing modules statically linked into the python binary \n");

static struct PyModuleDef _static_extension_utils_def = {
    PyModuleDef_HEAD_INIT,
    "_static_extension_utils_def",
    module_doc,
    0,
    _static_extension_utils_methods,
    _static_extension_utils_slots,
    nullptr,
    nullptr,
    nullptr};

PyMODINIT_FUNC PyInit__static_extension_utils(void) {
  return PyModuleDef_Init(&_static_extension_utils_def);
}
} // namespace
